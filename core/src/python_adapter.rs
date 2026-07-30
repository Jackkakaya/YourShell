//! `python3` / `python` / `pip` / `pip3`, forwarded to the embedded CPython.
//!
//! The interpreter itself is owned by C code compiled into the app target
//! (`python_host.c`), which has the real `Python.h`: the runtime is initialized
//! once (PyConfig, PYTHONHOME from the app bundle) and each command runs
//! through a small Python driver. This module only forwards argv and the pieces
//! of session state a resident interpreter cannot pick up on its own.
//!
//! Compiled only with the `python` cargo feature. The app embeds but does not
//! launch-link Python.xcframework; `python_host.c` resolves the C API with
//! `dlopen`/`dlsym` on the first Python-family command.

use std::ffi::{c_char, c_int, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    /// Provided by python_host.c in the app target.
    fn ys_python_run(argc: c_int, argv: *const *const c_char) -> c_int;
}

/// Make `pip` usable on iOS. `ensurepip` can't run — it shells out to a
/// subprocess, which iOS forbids (`OSError: ios does not support processes`) —
/// so bootstrap pip the way that actually works here: unzip the CPython-bundled
/// pip wheel into the writable site dir (`YOURSHELL_PY_SITE`, already on
/// `sys.path`). Idempotent, and best-effort: any failure just leaves
/// `python -m pip` to report "No module named pip".
fn ensure_pip_bootstrapped() {
    let Ok(site) = std::env::var("YOURSHELL_PY_SITE") else {
        return;
    };
    let site = std::path::PathBuf::from(site);

    let extract_matching_wheel = |directory: &std::path::Path, prefix: &str| {
        let wheel = std::fs::read_dir(directory)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".whl"))
            })?;
        let file = std::fs::File::open(wheel).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let _ = std::fs::create_dir_all(&site);
        archive.extract(&site).ok()
    };

    if !site.join("pip").is_dir() {
        if let Ok(home) = std::env::var("YOURSHELL_PYTHON_HOME") {
            let bundled = std::path::Path::new(&home).join("lib/python3.14/ensurepip/_bundled");
            let _ = extract_matching_wheel(&bundled, "pip-");
        }
    }

    // Pure-Python source distributions use setuptools.build_meta. iOS cannot
    // spawn pip's isolated build subprocess, so make that backend available in
    // the same writable site before invoking pip.
    if !site.join("setuptools").is_dir() {
        if let Ok(wheels) = std::env::var("YOURSHELL_PY_WHEELS") {
            let _ = extract_matching_wheel(std::path::Path::new(&wheels), "setuptools-");
        }
    }
}

/// The read-only app bundle isn't a writable install site, so a bare
/// `pip install X` has nowhere to write in the signed app bundle. Default to
/// CPython's standard user-site unless the user chose a destination. Unlike
/// `--target`, `--user` keeps prebundled packages visible to dependency
/// resolution, so pip will not attempt to rebuild NumPy from source on iOS.
fn inject_pip_target(argv: &mut Vec<String>) {
    if !argv.iter().any(|a| a == "install") {
        return;
    }
    let has_dest = argv.iter().any(|a| {
        a == "--target"
            || a == "-t"
            || a.starts_with("--target=")
            || a == "--prefix"
            || a == "--user"
    });
    if !has_dest {
        argv.push("--user".into());
    }
    // Force --no-build-isolation. iOS has no fork, so pip's build isolation runs
    // a *nested* `pip install <build-deps>` (pip-in-pip) which is fragile in the
    // in-process subprocess shim and dies silently (e.g. legacy setup.py
    // packages). Use the prebundled build backends directly instead
    // (setuptools/wheel/flit_core/hatchling are shipped and on sys.path).
    let has_iso = argv
        .iter()
        .any(|a| a == "--no-build-isolation" || a == "--build-isolation");
    if !has_iso {
        argv.push("--no-build-isolation".into());
    }

    // Mobile links commonly pause during Wi-Fi/cellular handoff. pip's short
    // desktop-oriented read timeout makes a healthy but slow link look like a
    // package incompatibility. Supply conservative defaults while preserving
    // every explicit user choice.
    let has_timeout = argv
        .iter()
        .any(|a| a == "--timeout" || a.starts_with("--timeout="));
    if !has_timeout {
        argv.extend(["--timeout".into(), "60".into()]);
    }
    let has_retries = argv
        .iter()
        .any(|a| a == "--retries" || a.starts_with("--retries="));
    if !has_retries {
        argv.extend(["--retries".into(), "5".into()]);
    }
    let has_resume_retries = argv
        .iter()
        .any(|a| a == "--resume-retries" || a.starts_with("--resume-retries="));
    if !has_resume_retries {
        argv.extend(["--resume-retries".into(), "5".into()]);
    }
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: CPython 3.14 (in-process)"))
}

fn python_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    let mut argv: Vec<String> = ctx.argv.to_vec();

    // `pip`/`pip3` run as `python -m pip …` — there is no standalone pip binary
    // on iOS. argv[0] is ignored by the driver (it reads sys.argv[1:]).
    if matches!(ctx.name, "pip" | "pip3") {
        ensure_pip_bootstrapped();
        let rest = if argv.is_empty() {
            Vec::new()
        } else {
            argv[1..].to_vec()
        };
        argv = vec![ctx.name.to_string(), "-m".into(), "pip".into()];
        argv.extend(rest);
        inject_pip_target(&mut argv);
    }

    // The resident interpreter froze `os.environ` at init, so the host's setenv
    // is invisible to it. Hand the exported env to the driver via a JSON file it
    // re-reads each command (path fixed at startup, captured in os.environ).
    if let Some(env_file) = std::env::var_os("YS_PY_ENV_FILE") {
        let esc = |s: &str| {
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        };
        let mut entries: Vec<String> = ctx
            .env
            .iter()
            .map(|(k, v)| format!("\"{}\":\"{}\"", esc(k), esc(v)))
            .collect();
        // A bare `python3` on a live terminal is a REPL; piped or redirected it
        // is a script. The fd cannot answer this — a session's stdin is always a
        // pipe — so the shell's own answer is forwarded.
        // Always write the value. CPython's os.environ assignment calls
        // setenv(), so merely omitting the key on a later redirected command
        // would retain "1" from a previous interactive invocation and turn
        // `python3 < script.py` into a REPL.
        entries.push(format!(
            "\"YS_STDIN_TTY\":\"{}\"",
            if ctx.stdin_is_interactive { "1" } else { "0" }
        ));
        let _ = std::fs::write(&env_file, format!("{{{}}}", entries.join(",")));
    }

    // Run CPython on a dedicated 16 MiB thread. tokio's blocking-pool threads
    // have a 2 MiB stack, which overflows on deep recursive deallocation of
    // nested object graphs (subtype_dealloc -> tuple_dealloc -> …), crashing
    // with a bus error in the GC. Desktop Python runs on the 8 MiB main thread;
    // this matches that headroom. fds/cwd/env are process-global and already
    // redirected on the (locked) calling thread, so the child inherits them.
    std::thread::Builder::new()
        .name("yourshell-python".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let cstrings: Vec<CString> = argv
                .iter()
                .map(|s| CString::new(s.as_str()).unwrap_or_default())
                .collect();
            let ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
            catch_unwind(AssertUnwindSafe(|| unsafe {
                ys_python_run(ptrs.len() as c_int, ptrs.as_ptr())
            }))
            .unwrap_or(134)
        })
        .ok()
        .and_then(|h| h.join().ok())
        .unwrap_or(134)
}

fn exec_python(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, python_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_python,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

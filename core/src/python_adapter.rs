//! Bridges CPython into brush as the `python3`/`python` builtins.
//!
//! The interpreter itself is owned by C code compiled into the app target
//! (`python_host.c`), which has the real `Python.h` — the runtime is
//! initialized once (PyConfig, PYTHONHOME from the app bundle) and each
//! command invocation runs in a fresh subinterpreter. This module only
//! prepares process state: it resolves the session's fds, working dir and
//! exported env, then calls the host under the shared process-state lock
//! (fds 0/1/2 and cwd are process-global, exactly like the uutils adapter).
//!
//! Compiled only with the `python` cargo feature: the symbols resolve when
//! the app links Python.xcframework; host test binaries build without it.

use std::ffi::{c_char, c_int, CString};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::panic::{catch_unwind, AssertUnwindSafe};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::uutils_adapter::process_state_lock;

unsafe extern "C" {
    /// Provided by python_host.c in the app target.
    fn ys_python_run(argc: c_int, argv: *const *const c_char) -> c_int;
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

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: CPython 3.14 (in-process, subinterpreter)"))
}

/// Make `pip` usable on iOS. `ensurepip` can't run — it shells out to a
/// subprocess, which iOS forbids (`OSError: ios does not support processes`) —
/// so we bootstrap pip the way it actually works here: unzip the CPython-bundled
/// pip wheel into the writable site dir (`YOURSHELL_PY_SITE`, already on
/// `sys.path`). Idempotent (skips if pip is already extracted) and best-effort:
/// any failure just leaves `python -m pip` to report "No module named pip".
fn ensure_pip_bootstrapped() {
    let Ok(site) = std::env::var("YOURSHELL_PY_SITE") else {
        return;
    };
    let site = std::path::PathBuf::from(site);
    if site.join("pip").is_dir() {
        return; // already bootstrapped
    }
    let Ok(home) = std::env::var("YOURSHELL_PYTHON_HOME") else {
        return;
    };
    let bundled = std::path::Path::new(&home).join("lib/python3.14/ensurepip/_bundled");
    let Ok(entries) = std::fs::read_dir(&bundled) else {
        return;
    };
    let wheel = entries.filter_map(Result::ok).map(|e| e.path()).find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("pip-") && n.ends_with(".whl"))
    });
    let Some(wheel) = wheel else {
        return;
    };
    if let Ok(f) = std::fs::File::open(&wheel) {
        if let Ok(mut zip) = zip::ZipArchive::new(f) {
            let _ = std::fs::create_dir_all(&site);
            let _ = zip.extract(&site);
        }
    }
}

/// The read-only app bundle isn't a writable install site, so a bare
/// `pip install X` has nowhere to write. Default to `--user` (writable
/// PYTHONUSERBASE, set by the host) UNLESS the user already chose a destination
/// (`--target`/`-t`/`--prefix`/`--user`). Only touches `install`.
///
/// `--user` (not `--target`) is deliberate: `--target` isolates pip's resolver,
/// so a dependency already provided by the read-only prebundled dir (e.g. numpy)
/// gets reinstalled from source → meson build → fails on iOS. `--user` resolves
/// against the whole environment, so prebundled numpy/pandas/lxml/… are seen as
/// installed and skipped. Requires a non-isolated interpreter with user-site
/// enabled (see python_host.c).
fn inject_pip_target(argv: &mut Vec<String>) {
    if !argv.iter().any(|a| a == "install") {
        return;
    }
    let has_dest = argv.iter().any(|a| {
        a == "--target" || a == "-t" || a.starts_with("--target=") || a == "--prefix" || a == "--user"
    });
    if !has_dest {
        argv.push("--user".into());
    }
    // Force --no-build-isolation. iOS has no fork, so pip's build isolation runs
    // a *nested* `pip install <build-deps>` (pip-in-pip) which is fragile in the
    // in-process subprocess shim and dies silently (e.g. legacy setup.py packages
    // like ppt). Instead use the prebundled build backends directly (setuptools/
    // wheel/flit_core/hatchling are shipped and on sys.path), so the PEP 517
    // build backend runs in-process without a nested pip. Skip if the user set it.
    let has_iso = argv.iter().any(|a| a == "--no-build-isolation" || a == "--build-isolation");
    if !has_iso {
        argv.push("--no-build-isolation".into());
    }
}

fn exec_python(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let mut argv: Vec<String> = args.iter().map(ToString::to_string).collect();

        // `pip`/`pip3` run as `python -m pip …` in the embedded interpreter
        // (there's no standalone pip executable on iOS). argv[0] is ignored by
        // the driver (it reads sys.argv[1:]), so inject `-m pip` after it.
        // First make sure pip exists (bootstrap from the bundled wheel) and that
        // installs land in the writable site dir — both no-ops after the first run.
        if matches!(context.command_name.as_str(), "pip" | "pip3") {
            ensure_pip_bootstrapped();
            let rest = if argv.is_empty() { Vec::new() } else { argv[1..].to_vec() };
            argv = vec![context.command_name.clone(), "-m".into(), "pip".into()];
            argv.extend(rest);
            inject_pip_target(&mut argv);
        }

        let fds: [Option<std::os::fd::OwnedFd>; 3] = [0, 1, 2].map(|n| {
            context.try_fd(n.into()).and_then(|f| {
                f.try_borrow_as_fd()
                    .ok()
                    .and_then(|bfd| bfd.try_clone_to_owned().ok())
            })
        });
        let cwd = context.shell.working_dir().to_path_buf();
        let exported: Vec<(String, String)> = context
            .shell
            .env()
            .iter_exported()
            .filter(|(_, v)| v.value().is_set())
            .map(|(k, v)| (k.clone(), v.value().to_cow_str(context.shell).into_owned()))
            .collect();

        // Authoritative interactive check: stdin is interactive only when it
        // was NOT explicitly specified for this command (no pipe/redirect) —
        // it's inherited from the session. Non-blocking peeks race with
        // pipeline writers and dev/ino comparison is unreliable for pipes.
        let stdin_redirected = context.params.is_fd_specified(0.into());

        let code = tokio::task::spawn_blocking(move || {
            let mut fds = fds;
            let mut exported = exported;
            let stdin_buf: Option<Vec<u8>> = if stdin_redirected {
                // Piped/redirected: drain fully and run as a script.
                fds[0].take().map(|fd| {
                    let mut f = std::fs::File::from(fd);
                    let mut buf = Vec::new();
                    let _ = f.read_to_end(&mut buf);
                    buf
                })
            } else {
                // Inherited session stdin: a live terminal -> REPL for bare
                // `python3` (the driver reads fd 0 directly).
                exported.push(("YS_STDIN_TTY".to_string(), "1".to_string()));
                None
            };
            run_locked(argv, fds, stdin_buf, &cwd, &exported)
        })
        .await
        .unwrap_or(126);

        #[expect(clippy::cast_sign_loss)]
        Ok(ExecutionResult::new((code & 0xff) as u8))
    })
}

fn run_locked(
    argv: Vec<String>,
    mut fds: [Option<std::os::fd::OwnedFd>; 3],
    stdin_buf: Option<Vec<u8>>,
    cwd: &std::path::Path,
    exported: &[(String, String)],
) -> i32 {
    let _guard = process_state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if let Some(buf) = &stdin_buf {
        fds[0] = crate::uutils_adapter::buffered_stdin_fd(buf);
    }

    let saved_cwd = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(cwd);

    let saved_env: Vec<(String, Option<std::ffi::OsString>)> = exported
        .iter()
        .map(|(k, v)| {
            let prev = std::env::var_os(k);
            std::env::set_var(k, v);
            (k.clone(), prev)
        })
        .collect();

    // The persistent interpreter froze os.environ at init, so setenv above is
    // invisible to it. Hand the exported env to the driver via a JSON file it
    // reads each command (path fixed at startup, captured in os.environ).
    if let Some(env_file) = std::env::var_os("YS_PY_ENV_FILE") {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        let entries: Vec<String> = exported
            .iter()
            .map(|(k, v)| format!("\"{}\":\"{}\"", esc(k), esc(v)))
            .collect();
        let _ = std::fs::write(&env_file, format!("{{{}}}", entries.join(",")));
    }

    let mut saved_fds: Vec<(i32, i32)> = Vec::new();
    for (target, fd) in fds.iter().enumerate() {
        if let Some(fd) = fd {
            let target = target as i32;
            let saved = unsafe { libc::dup(target) };
            if saved >= 0 {
                unsafe { libc::dup2(fd.as_raw_fd(), target) };
                saved_fds.push((target, saved));
            }
        }
    }

    // Run CPython on a dedicated 16 MiB thread. tokio's blocking-pool threads
    // have a 2 MiB stack, which overflows on deep recursive deallocation of
    // nested object graphs (subtype_dealloc -> tuple_dealloc -> ...), crashing
    // with a bus error in the GC. Desktop Python runs on the 8 MiB main thread;
    // we match that headroom. fds/cwd/env are already redirected on this
    // (locked) thread and are process-global, so the child inherits them.
    let argv_owned = argv;
    let code = std::thread::Builder::new()
        .name("yourshell-python".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let cstrings: Vec<CString> = argv_owned
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
        .unwrap_or(134);

    for (target, saved) in saved_fds {
        unsafe {
            libc::dup2(saved, target);
            libc::close(saved);
        }
    }
    for (k, prev) in saved_env {
        match prev {
            Some(v) => std::env::set_var(&k, v),
            None => std::env::remove_var(&k),
        }
    }
    if let Some(prev) = saved_cwd {
        let _ = std::env::set_current_dir(prev);
    }

    code
}

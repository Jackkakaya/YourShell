//! Bridges uutils/coreutils commands into brush as in-process builtins.
//!
//! uutils utilities read/write the *process* stdio, cwd and environment, while
//! each brush Shell instance carries its own fd table, working dir and env.
//! The adapter reconciles the two per invocation, under a global lock:
//!
//!   1. chdir to the shell's working dir
//!   2. export the shell's exported vars into the process env
//!   3. dup2 the session's fd 0/1/2 over the process fds
//!   4. call the utility's `uumain` (via brush-coreutils-builtins' registry),
//!      wrapped in `catch_unwind`
//!   5. flush stdout, then restore fds, env and cwd
//!
//! The lock serializes uutils commands across sessions; brush builtins and
//! library-backed commands are unaffected. High-traffic utilities can later
//! be migrated to context-injected implementations to regain concurrency.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

/// Commands whose brush-native builtin should win over the uutils version
/// (shell semantics, shell-state awareness, no serialization cost).
const KEEP_BRUSH: &[&str] = &["echo", "printf", "pwd", "test", "true", "false"];

/// Commands we deliberately do not expose yet: `yes` streams forever and we
/// have no interrupt delivery until the M1 terminal work lands; `more` is an
/// interactive pager that assumes a controlling tty.
const SKIP: &[&str] = &["yes", "more"];

/// Serializes every mutation of process-global state (fds 0/1/2, cwd, env).
/// Shared by the uutils adapter and the Python runner — anything that dup2s
/// must hold this lock.
pub(crate) fn process_state_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

fn registry() -> &'static HashMap<String, fn(Vec<OsString>) -> i32> {
    static REGISTRY: OnceLock<HashMap<String, fn(Vec<OsString>) -> i32>> = OnceLock::new();
    REGISTRY.get_or_init(brush_coreutils_builtins::bundled_commands)
}

/// Names to register, after policy filtering.
pub fn command_names() -> Vec<String> {
    let mut names: Vec<String> = registry()
        .keys()
        .filter(|n| !KEEP_BRUSH.contains(&n.as_str()) && !SKIP.contains(&n.as_str()))
        .cloned()
        .collect();
    names.sort();
    names
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_uutils,
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
    Ok(format!("{name}: coreutils command (uutils, in-process)"))
}

fn exec_uutils(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let name = context.command_name.clone();

        // brush passes the command name as args[0], matching uumain's argv
        // convention directly.
        let argv: Vec<OsString> = args.iter().map(|a| OsString::from(a.to_string())).collect();

        // Resolve session state on the async side; the blocking side must be
        // 'static. Missing fds fall through to the process defaults.
        let fds: [Option<std::os::fd::OwnedFd>; 3] = [0, 1, 2].map(|n| {
            context.try_fd(n.into()).and_then(|f| {
                f.try_borrow_as_fd()
                    .ok()
                    .and_then(|bfd| bfd.try_clone_to_owned().ok())
            })
        });

        // Distinguish "stdin is the session's interactive stdin" from "stdin
        // was redirected / comes from a pipeline stage". Redirected stdin is
        // fully drained BEFORE taking the global lock — otherwise two uutils
        // stages in one pipeline deadlock (downstream grabs the lock, blocks
        // reading input that upstream can't produce without the lock).
        let session_stdin_fd: Option<i32> = context
            .shell
            .open_files()
            .try_fd(0.into())
            .and_then(|f| f.try_borrow_as_fd().ok().map(|b| b.as_raw_fd()));
        let stdin_is_interactive = match (&fds[0], session_stdin_fd) {
            (Some(fd0), Some(base)) => {
                // Same underlying description as the session's base stdin?
                // Arc-shared OpenFiles resolve to the same raw fd.
                fd0.as_raw_fd() == base || raw_fd_same_file(fd0.as_raw_fd(), base)
            }
            (None, _) => true,
            _ => false,
        };
        let cwd = context.shell.working_dir().to_path_buf();
        let exported: Vec<(String, String)> = context
            .shell
            .env()
            .iter_exported()
            .filter(|(_, v)| v.value().is_set())
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.value().to_cow_str(context.shell).into_owned(),
                )
            })
            .collect();

        let code = tokio::task::spawn_blocking(move || {
            let mut fds = fds;
            // Drain redirected stdin to a buffer while NOT holding the lock;
            // upstream pipeline stages (brush builtins or other uutils
            // commands) are free to run and close their write end.
            let stdin_buf: Option<Vec<u8>> = if stdin_is_interactive {
                None
            } else {
                fds[0].take().map(|fd| {
                    let mut f = std::fs::File::from(fd);
                    let mut buf = Vec::new();
                    let _ = f.read_to_end(&mut buf);
                    buf
                })
            };
            run_locked(&name, argv, fds, stdin_buf, &cwd, &exported)
        })
        .await
        .unwrap_or(126);

        #[expect(clippy::cast_sign_loss)]
        Ok(ExecutionResult::new((code & 0xff) as u8))
    })
}

/// Compares two fds by file description identity (dev + inode), so a dup'd
/// clone of the session stdin still registers as "interactive".
pub(crate) fn raw_fd_same_file(a: i32, b: i32) -> bool {
    unsafe {
        let mut sa: libc::stat = std::mem::zeroed();
        let mut sb: libc::stat = std::mem::zeroed();
        libc::fstat(a, &raw mut sa) == 0
            && libc::fstat(b, &raw mut sb) == 0
            && sa.st_dev == sb.st_dev
            && sa.st_ino == sb.st_ino
    }
}

/// Materializes a drained stdin buffer as an unlinked temp file fd.
pub(crate) fn buffered_stdin_fd(buf: &[u8]) -> Option<std::os::fd::OwnedFd> {
    use std::io::{Seek, SeekFrom, Write};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "yourshell_stdin_{}_{n}",
        std::process::id()
    ));
    let mut f = std::fs::File::options()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    let _ = std::fs::remove_file(&path);
    f.write_all(buf).ok()?;
    f.seek(SeekFrom::Start(0)).ok()?;
    Some(std::os::fd::OwnedFd::from(f))
}

fn run_locked(
    name: &str,
    argv: Vec<OsString>,
    mut fds: [Option<std::os::fd::OwnedFd>; 3],
    stdin_buf: Option<Vec<u8>>,
    cwd: &PathBuf,
    exported: &[(String, String)],
) -> i32 {
    let _guard = process_state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let Some(func) = registry().get(name).copied() else {
        return 127;
    };

    if let Some(buf) = &stdin_buf {
        fds[0] = buffered_stdin_fd(buf);
    }

    // uucore's exit code is a process-global atomic that utilities set on
    // error and never clear; without a reset, one failed `ls` makes every
    // later success report the stale nonzero code.
    uucore::error::set_exit_code(0);

    let saved_cwd = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(cwd);

    let saved_env: Vec<(String, Option<OsString>)> = exported
        .iter()
        .map(|(k, v)| {
            let prev = std::env::var_os(k);
            std::env::set_var(k, v);
            (k.clone(), prev)
        })
        .collect();

    // Redirect process fd 0/1/2 to the session's fds, remembering originals.
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

    let code = catch_unwind(AssertUnwindSafe(|| func(argv))).unwrap_or(134);

    // The wrapper flushes stdout itself, but flush again defensively before
    // the fds are swapped back.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let _ = std::io::Write::flush(&mut std::io::stderr());

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

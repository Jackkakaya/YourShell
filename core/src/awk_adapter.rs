//! Bridges one-true-awk (Kernighan's awk, vendored C in vendor/awk) into brush
//! as the `awk` builtin.
//!
//! The awk core is C compiled into this crate (see build.rs). Its entry point
//! `ys_awk_main(argc, argv)` reads the process-global `stdin` FILE* and writes
//! the process-global `stdout`/`stderr`, exactly like the uutils and Python
//! adapters. So this module reconciles the shell's per-session fd table with
//! process globals under the shared `process_state_lock`:
//!
//!   1. resolve the session's fd 0/1/2, cwd and exported env
//!   2. drain a redirected/piped stdin to a temp fd BEFORE taking the lock
//!      (otherwise two locked stages in one pipeline deadlock — same reasoning
//!      as uutils_adapter)
//!   3. under the lock: chdir, export env, dup2 the fds over 0/1/2
//!   4. call `ys_awk_main`, wrapped in catch_unwind
//!   5. flush stdout/stderr, restore fds/env/cwd
//!
//! `ys_awk_main` resets awk's process-global input/stream state on entry (see
//! main.c / lib.c `ys_awk_reset_io`), so repeated invocations in this
//! long-lived process behave like fresh awk processes. Fatal awk errors
//! longjmp back out instead of exiting the host process. awk's `system()` and
//! `cmd | getline` / `print | cmd` are stubbed (no fork/exec on iOS).

use std::ffi::{c_char, c_int, CString};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::panic::{catch_unwind, AssertUnwindSafe};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::uutils_adapter::{buffered_stdin_fd, process_state_lock};

unsafe extern "C" {
    /// one-true-awk's renamed `main`, provided by vendor/awk/main.c.
    fn ys_awk_main(argc: c_int, argv: *const *const c_char) -> c_int;
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_awk,
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
    Ok(format!("{name}: awk (one-true-awk, in-process)"))
}

fn exec_awk(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        // brush passes the command name as args[0], matching argv convention.
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();

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

        // stdin is interactive only when it was NOT specified for this command;
        // a redirect/pipe means we drain it to a temp fd so awk reads a stable
        // description and pipelines don't deadlock on the process lock.
        let stdin_redirected = context.params.is_fd_specified(0.into());

        let code = tokio::task::spawn_blocking(move || {
            let mut fds = fds;
            let stdin_buf: Option<Vec<u8>> = if stdin_redirected {
                fds[0].take().map(|fd| {
                    let mut f = std::fs::File::from(fd);
                    let mut buf = Vec::new();
                    let _ = f.read_to_end(&mut buf);
                    buf
                })
            } else {
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
        fds[0] = buffered_stdin_fd(buf);
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

    let cstrings: Vec<CString> = argv
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap_or_default())
        .collect();
    let ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
    let code = catch_unwind(AssertUnwindSafe(|| unsafe {
        ys_awk_main(ptrs.len() as c_int, ptrs.as_ptr())
    }))
    .unwrap_or(134);

    // awk flushes stdout itself, but flush defensively before swapping fds back.
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

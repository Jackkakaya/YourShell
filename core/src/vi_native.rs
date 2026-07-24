//! `vi` backed by the vendored nextvi (real vi, ISC). Like the uutils/python
//! adapters, it redirects the session's fds onto process 0/1/2 under the shared
//! process-state lock, then calls nextvi's entry. It also performs the
//! alternate-screen handshake so the Swift side switches to raw key passthrough
//! for the duration.

use std::ffi::{c_char, c_int, CString};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::panic::{catch_unwind, AssertUnwindSafe};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::uutils_adapter::process_state_lock;

unsafe extern "C" {
    fn ys_nextvi_main(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_vi,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

fn content(
    _name: &str,
    _t: ContentType,
    _o: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok("vi: nextvi (real vi, in-process)".to_string())
}

fn exec_vi(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        // argv: ["vi", file...] — nextvi reads argv[0] as the program name.
        let mut argv: Vec<String> = vec!["vi".to_string()];
        argv.extend(args.iter().skip(1).map(ToString::to_string));

        let fds: [Option<std::os::fd::OwnedFd>; 3] = [0, 1, 2].map(|n| {
            context.try_fd(n.into()).and_then(|f| {
                f.try_borrow_as_fd()
                    .ok()
                    .and_then(|b| b.try_clone_to_owned().ok())
            })
        });
        let cwd = context.shell.working_dir().to_path_buf();

        let code = tokio::task::spawn_blocking(move || run_locked(argv, fds, &cwd))
            .await
            .unwrap_or(1);
        Ok(ExecutionResult::new((code & 0xff) as u8))
    })
}

fn run_locked(
    argv: Vec<String>,
    fds: [Option<std::os::fd::OwnedFd>; 3],
    cwd: &std::path::Path,
) -> i32 {
    let _guard = process_state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let saved_cwd = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(cwd);

    // Redirect session fds onto process 0/1/2 (nextvi uses STDIN/STDOUT_FILENO).
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

    // Enter the alternate screen (flips the Swift side into raw key passthrough).
    {
        let mut out = unsafe { std::fs::File::from(std::os::fd::OwnedFd::from_raw_fd_dup(1)) };
        let _ = out.write_all(b"\x1b[?1049h");
        let _ = out.flush();
    }

    // Run nextvi on a dedicated large stack (it recurses through regex/undo).
    let code = std::thread::Builder::new()
        .name("yourshell-vi".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let mut cstrings: Vec<CString> = argv
                .iter()
                .map(|s| CString::new(s.as_str()).unwrap_or_default())
                .collect();
            let mut ptrs: Vec<*mut c_char> =
                cstrings.iter_mut().map(|c| c.as_ptr().cast_mut()).collect();
            catch_unwind(AssertUnwindSafe(|| unsafe {
                ys_nextvi_main(ptrs.len() as c_int, ptrs.as_mut_ptr())
            }))
            .unwrap_or(1)
        })
        .ok()
        .and_then(|h| h.join().ok())
        .unwrap_or(1);

    // Leave the alternate screen.
    {
        let mut out = unsafe { std::fs::File::from(std::os::fd::OwnedFd::from_raw_fd_dup(1)) };
        let _ = out.write_all(b"\x1b[?1049l");
        let _ = out.flush();
    }

    for (target, saved) in saved_fds {
        unsafe {
            libc::dup2(saved, target);
            libc::close(saved);
        }
    }
    if let Some(prev) = saved_cwd {
        let _ = std::env::set_current_dir(prev);
    }
    code
}

/// Helper: duplicate a raw fd into an OwnedFd so writing to it via File doesn't
/// close the real fd on drop.
trait FromRawFdDup {
    unsafe fn from_raw_fd_dup(fd: i32) -> std::os::fd::OwnedFd;
}
impl FromRawFdDup for std::os::fd::OwnedFd {
    unsafe fn from_raw_fd_dup(fd: i32) -> std::os::fd::OwnedFd {
        use std::os::fd::FromRawFd;
        std::os::fd::OwnedFd::from_raw_fd(libc::dup(fd))
    }
}

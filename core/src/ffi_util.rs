//! Shared fd / error helpers for the in-process network adapters (ssh, scp,
//! sftp, mosh). These were previously copy-pasted (with subtly different
//! behavior) across `ssh_adapter`, `sftp_adapter` and `mosh_adapter`.

use std::os::fd::{OwnedFd, RawFd};

use brush_core::extensions::DefaultShellExtensions;
use brush_core::{ExecutionContext, ExecutionResult};

/// Clones one of the session's fds (0/1/2) as an owned fd, or None if absent.
pub(crate) fn borrow_fd(
    context: &ExecutionContext<'_, DefaultShellExtensions>,
    n: i32,
) -> Option<OwnedFd> {
    context.try_fd(n.into()).and_then(|f| {
        f.try_borrow_as_fd()
            .ok()
            .and_then(|bfd| bfd.try_clone_to_owned().ok())
    })
}

/// Writes all of `data` to a raw fd, looping on short writes.
///
/// SAFETY: `fd` must be a valid, writable file descriptor for the duration.
pub(crate) fn write_all_fd(fd: RawFd, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n <= 0 {
            return Err(std::io::Error::last_os_error());
        }
        data = &data[n as usize..];
    }
    Ok(())
}

/// Best-effort write of a short message (typically an error) to a raw fd.
pub(crate) fn eprint_fd(fd: RawFd, msg: &str) {
    let _ = write_all_fd(fd, msg.as_bytes());
}

/// Prints `msg` to the command's stderr and returns an `ExecutionResult` with
/// the given exit code — the standard "parse/setup error" path for adapters.
pub(crate) fn fail(
    context: &ExecutionContext<'_, DefaultShellExtensions>,
    msg: &str,
    code: u8,
) -> Result<ExecutionResult, brush_core::Error> {
    let mut err = context.stderr();
    let _ = std::io::Write::write_all(&mut err, msg.as_bytes());
    let _ = std::io::Write::write_all(&mut err, b"\n");
    Ok(ExecutionResult::new(code))
}

/// Saves the status flags (esp. `O_NONBLOCK`) of some fds and restores them on
/// drop. The tokio `AsyncFd` wrappers set `O_NONBLOCK` on the shared file
/// descriptions; without restoring, the shell's blocking reader/writer would be
/// left non-blocking after an interactive session and hit spurious `EAGAIN`.
pub(crate) struct FdFlagsGuard {
    saved: Vec<(RawFd, i32)>,
}

impl FdFlagsGuard {
    pub(crate) fn capture(fds: &[RawFd]) -> Self {
        let saved = fds
            .iter()
            .filter_map(|&fd| {
                // SAFETY: F_GETFL on any fd is safe; an invalid fd returns -1.
                let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                (flags >= 0).then_some((fd, flags))
            })
            .collect();
        Self { saved }
    }
}

impl Drop for FdFlagsGuard {
    fn drop(&mut self) {
        for &(fd, flags) in &self.saved {
            // SAFETY: restoring previously-read flags on the same fd.
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, flags);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};

    #[test]
    fn raw_fd_write_and_error_paths() {
        let (mut reader, writer) = std::io::pipe().unwrap();
        write_all_fd(writer.as_raw_fd(), b"complete-write").unwrap();
        drop(writer);
        let mut value = String::new();
        reader.read_to_string(&mut value).unwrap();
        assert_eq!(value, "complete-write");

        let error = write_all_fd(-1, b"x").unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EBADF));
        // Best-effort error printing must contain the same invalid-fd path.
        eprint_fd(-1, "ignored");
    }

    #[test]
    fn fd_flags_guard_restores_original_status_flags() {
        let (reader, writer) = std::io::pipe().unwrap();
        let fd = reader.as_raw_fd();
        let original = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        {
            let _guard = FdFlagsGuard::capture(&[fd, -1]);
            let changed = original | libc::O_NONBLOCK;
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFL, changed) }, 0);
            assert_ne!(
                unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
                0
            );
        }
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFL) }, original);
        drop(writer);
        drop(reader);
    }

    #[test]
    fn guard_tolerates_fd_closed_before_drop() {
        let (reader, writer) = std::io::pipe().unwrap();
        let raw = reader.as_raw_fd();
        let guard = FdFlagsGuard::capture(&[raw]);
        let raw = std::os::fd::IntoRawFd::into_raw_fd(reader);
        // SAFETY: raw was just detached from `reader`; this closes it once.
        drop(unsafe { std::fs::File::from_raw_fd(raw) });
        drop(guard);
        drop(writer);
    }
}

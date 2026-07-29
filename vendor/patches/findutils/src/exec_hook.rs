// YourShell patch: pluggable command execution for `find -exec` and `xargs`.
//
// Upstream runs child commands with `std::process::Command`. iOS forbids
// fork/exec entirely (`[Errno 45] ios does not support processes`), so both
// commands would be dead on arrival there — and they are two of the most
// frequently used tools in the whole shell.
//
// The seam is tiny (one call site in xargs, two in find), so instead of
// reimplementing either tool we let the embedder install a hook that runs the
// argv however it can — in YourShell's case, in a fresh in-process subshell.
// A subshell is also the semantically correct target: `xargs` and `find -exec`
// spawn independent processes upstream, so state must not leak back.
//
// With no hook installed the behaviour is unchanged (`std::process::Command`),
// which keeps upstream's own test-suite and desktop builds working.

use std::ffi::OsString;
use std::sync::OnceLock;

/// Runs `argv` with `env` (and `cwd`, for `find -execdir`), returning the
/// child's exit code. A negative return means "terminated by a signal"
/// (`-signum`), matching `ExitStatus::signal()`.
pub type ExecFn = fn(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    cwd: Option<&std::path::Path>,
    close_stdin: bool,
) -> i32;

static HOOK: OnceLock<ExecFn> = OnceLock::new();

/// Installs the embedder's command runner. First call wins; later calls are
/// ignored so a racing session cannot swap the runner mid-command.
pub fn set_exec_hook(f: ExecFn) {
    let _ = HOOK.set(f);
}

/// True when an embedder took over execution.
#[must_use]
pub fn has_hook() -> bool {
    HOOK.get().is_some()
}

/// Executes `argv`, via the hook when one is installed and via
/// `std::process::Command` otherwise.
#[must_use]
pub fn run(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    cwd: Option<&std::path::Path>,
    close_stdin: bool,
) -> i32 {
    if let Some(f) = HOOK.get() {
        return f(argv, env, cwd, close_stdin);
    }
    run_with_process(argv, env, cwd, close_stdin)
}

fn run_with_process(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    cwd: Option<&std::path::Path>,
    close_stdin: bool,
) -> i32 {
    use std::process::{Command, Stdio};
    let Some((exe, args)) = argv.split_first() else {
        return 127;
    };
    let mut cmd = Command::new(exe);
    cmd.args(args).env_clear().envs(env.iter().cloned());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    if close_stdin {
        cmd.stdin(Stdio::null());
    }
    match cmd.status() {
        Ok(status) => {
            if let Some(code) = status.code() {
                code
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal().map_or(1, |s| -s)
                }
                #[cfg(not(unix))]
                {
                    1
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 127,
        Err(_) => 126,
    }
}

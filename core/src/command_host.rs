//! The one place that reconciles a shell session with a process-shaped command.
//!
//! Almost every command implementation we adopt (uutils coreutils, findutils,
//! one-true-awk, CPython) is written to own a process: it reads `argv`, prints
//! to the process's fd 1, resolves relative paths against the process's cwd and
//! reads the process's environment. A shell session, by contrast, carries its
//! own fd table, working directory and environment, and several sessions live
//! in this one app process.
//!
//! Bridging those two is a fixed, fiddly sequence — take the lock, chdir,
//! setenv, dup2, run, flush, then undo all of it in reverse. It was previously
//! copy-pasted into four adapters, which is exactly the kind of duplication
//! that eventually diverges in the *restore* half and silently corrupts every
//! command that runs afterwards. It lives here once.
//!
//! Adopting a command is therefore: write a `fn(name, argv, env) -> i32` that
//! forwards to the upstream entry point, and hand it to [`dispatch`]. No flag
//! parsing, no command logic — those stay in the upstream crate, which is the
//! whole point of adopting it.

use std::ffi::{c_char, c_int, CString, OsString};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

/// Everything a forwarded command may need about the invocation. Passed as a
/// struct rather than growing the parameter list, since adopting each new
/// command tends to surface one more thing it wants to know.
#[allow(dead_code)]
pub(crate) struct CmdCtx<'a> {
    /// The invoked command name. Multi-command crates (uutils ships 74
    /// utilities behind one registry) dispatch on it; single-command adapters
    /// ignore it.
    pub name: &'a str,
    /// Includes `argv[0]`, matching what a real `main` receives.
    pub argv: &'a [String],
    /// The session's exported environment. Already applied to the process by
    /// the time this runs; passed through as well because some runtimes (the
    /// embedded CPython) froze `environ` at startup and need it by another
    /// route.
    pub env: &'a [(String, String)],
    /// The session's working directory — also already applied via `chdir`.
    pub cwd: &'a Path,
    /// False when stdin was redirected or piped for this command. Runtimes that
    /// behave differently on a terminal (a bare `python3` becomes a REPL) need
    /// this; the raw fd cannot answer it, because a session's stdin is always a
    /// pipe.
    pub stdin_is_interactive: bool,
}

/// A command's entry point, normalized. Returns the exit code.
pub(crate) type CommandMain = fn(&CmdCtx<'_>) -> i32;

/// Normalized ABI used by vendored C command frontends.
pub(crate) type CArgvMain = unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int;

/// Builds a conventional null-terminated C argv and calls an upstream CLI.
/// Process argv cannot contain NUL bytes, so reject those with the standard
/// shell "cannot execute" status instead of silently changing the argument.
pub(crate) fn run_c_argv(ctx: &CmdCtx<'_>, main: CArgvMain) -> i32 {
    let owned: Result<Vec<CString>, _> = ctx
        .argv
        .iter()
        .map(|arg| CString::new(arg.as_bytes()))
        .collect();
    let mut owned = match owned {
        Ok(argv) => argv,
        Err(_) => return 126,
    };
    let mut argv: Vec<*mut c_char> = owned
        .iter_mut()
        .map(|arg| arg.as_ptr().cast_mut())
        .collect();
    argv.push(std::ptr::null_mut());
    unsafe { main(owned.len() as c_int, argv.as_mut_ptr()) as i32 }
}

/// Classification rule for adapters: a `CommandMain` is process-shaped and
/// therefore must run under `process_state_lock`. Commands that can consume
/// `ExecutionContext` directly must not be routed through this type; doing so
/// would reintroduce the global fd/cwd/env bridge even if their implementation
/// itself is otherwise session-safe.

/// Serializes every mutation of process-global state (fds 0/1/2, cwd, env).
/// Any command that dup2s must hold this.
///
/// KNOWN LIMITATION (single-session-safe): a command reading the *interactive*
/// stdin (a bare `cat` waiting on the keyboard) holds this for its whole run,
/// because its stdin is not drained. With one terminal session that is
/// harmless. With several — and the agent's `terminal` tool is a second session
/// — such a command blocks the others until it sees EOF. Fixing that means
/// per-session fd isolation rather than one process-global lock, i.e. adopting
/// commands whose entry points accept injected I/O (`findutils`'s `find` does)
/// so they never need this lock at all.
pub(crate) fn process_state_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

/// Re-asserts `SIGPIPE -> SIG_IGN`.
///
/// `ashell_session_new` sets this once at startup, because as a staticlib in a
/// Swift app there is no Rust `main` to do it — without it, a command writing to
/// a pipe whose reader has gone away kills the entire app, taking the user's
/// conversation with it.
///
/// Setting it once is not enough: vendored C libraries reset it behind our back.
/// libgit2 in particular does — a `git init && git add && git commit` followed by
/// any early-exiting pipe reader reliably killed the app, and it took bisecting
/// the selftest battery down to those two commands to see it, because in
/// isolation either one is harmless.
///
/// So treat the signal disposition as process state the same way fds, cwd and
/// env are treated: re-establish it around every command rather than trusting
/// that nothing touched it.
pub(crate) fn ensure_sigpipe_ignored() {
    // SAFETY: `signal` with SIG_IGN is async-signal-safe and idempotent.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Materializes a drained stdin buffer as an unlinked temp file fd, so a
/// process-shaped command can `read(0)` it normally.
pub(crate) fn buffered_stdin_fd(buf: &[u8]) -> Option<std::os::fd::OwnedFd> {
    use std::io::{Seek, SeekFrom, Write};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("yourshell_stdin_{}_{n}", std::process::id()));
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

/// Runs `main` as a builtin: resolves the session's fds/cwd/env, moves the work
/// off the async thread, and applies the process-state dance around the call.
///
/// This is the whole integration surface. An adapter is a `registration()`
/// returning brush's `Registration` plus a one-line `exec` that calls this —
/// brush takes a bare `fn` pointer for `execute_func`, so the per-command entry
/// cannot be captured in a closure and each adapter needs that one small shim.
pub(crate) fn dispatch<'a>(
    context: ExecutionContext<'a, DefaultShellExtensions>,
    args: Vec<CommandArg>,
    main: CommandMain,
) -> BoxFuture<'a, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let name = context.command_name.clone();
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();

        let fds: [Option<std::os::fd::OwnedFd>; 3] = [0, 1, 2].map(|n| {
            context.try_fd(n.into()).and_then(|f| {
                f.try_borrow_as_fd()
                    .ok()
                    .and_then(|b| b.try_clone_to_owned().ok())
            })
        });

        // Distinguish "stdin is the session's interactive stdin" from "stdin was
        // redirected or comes from a pipeline stage". Redirected stdin is fully
        // drained BEFORE taking the lock — otherwise two stages of one pipeline
        // deadlock, the downstream holding the lock while blocked on input the
        // upstream cannot produce without it.
        //
        // brush's authoritative "was fd 0 specified?" is the right signal: a
        // dev+inode comparison is unreliable for pipes (two distinct pipes can
        // share dev/ino), and a false "interactive" verdict reintroduces exactly
        // the deadlock this dance exists to avoid.
        let stdin_is_interactive = !context.params.is_fd_specified(0.into());
        let cwd = context.shell.working_dir().to_path_buf();
        let exported: Vec<(String, String)> = context
            .shell
            .env()
            .iter_exported()
            .filter(|(_, v)| v.value().is_set())
            .map(|(k, v)| (k.clone(), v.value().to_cow_str(context.shell).into_owned()))
            .collect();

        let code = tokio::task::spawn_blocking(move || {
            let mut fds = fds;
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
            run_locked(
                &name,
                &argv,
                fds,
                stdin_buf,
                &cwd,
                &exported,
                stdin_is_interactive,
                main,
            )
        })
        .await
        .unwrap_or(126);

        #[expect(clippy::cast_sign_loss)]
        Ok(ExecutionResult::new((code & 0xff) as u8))
    })
}

/// The process-state dance. Order matters, and every step is undone in reverse
/// even when the command panics — a leaked fd, env var or cwd silently breaks
/// every command that runs after it.
#[allow(clippy::too_many_arguments)]
fn run_locked(
    name: &str,
    argv: &[String],
    mut fds: [Option<std::os::fd::OwnedFd>; 3],
    stdin_buf: Option<Vec<u8>>,
    cwd: &Path,
    exported: &[(String, String)],
    stdin_is_interactive: bool,
    main: CommandMain,
) -> i32 {
    let _guard = process_state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if let Some(buf) = &stdin_buf {
        fds[0] = buffered_stdin_fd(buf);
    }

    let saved_cwd: Option<PathBuf> = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(cwd);

    let saved_env: Vec<(String, Option<OsString>)> = exported
        .iter()
        .map(|(k, v)| {
            let prev = std::env::var_os(k);
            std::env::set_var(k, v);
            (k.clone(), prev)
        })
        .collect();

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

    // catch_unwind contains panics from vendored C and third-party Rust so the
    // restore below still runs. It cannot contain a `std::process::exit` — that
    // is why adopting a command includes auditing its source for one.
    let ctx = CmdCtx {
        name,
        argv,
        env: exported,
        cwd,
        stdin_is_interactive,
    };
    // Before: some earlier command may have reset it (libgit2 does).
    ensure_sigpipe_ignored();
    let code = catch_unwind(AssertUnwindSafe(|| main(&ctx))).unwrap_or(134);
    // After: this command may have reset it for the next one.
    ensure_sigpipe_ignored();

    // Rust's stdio handles and libc's FILE* buffers are independent. Native C
    // CLIs (awk, jq, sqlite3, and future bsdtar/curl) write through FILE*;
    // restoring fd 1/2 before flushing would send their buffered output into
    // the next command's pipes.
    unsafe {
        libc::fflush(std::ptr::null_mut());
    }
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

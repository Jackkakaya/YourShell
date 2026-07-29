//! FFI surface for the iOS shell MVP.
//!
//! Each session owns a dedicated OS thread running a current-thread tokio
//! runtime with one `brush_core::Shell` instance. The shell's fd 0/1/2 are
//! wired to real pipes; a reader thread pumps combined stdout/stderr bytes to
//! a Swift callback. cwd/env live in the Shell instance, so multiple sessions
//! in one process stay isolated.

mod awk_adapter;
mod builtins_extra;
mod command_host;
mod commands_ext;
mod curl_adapter;
mod diffutils_adapter;
mod editor;
mod ffi_util;
mod findutils_adapter;
mod git_adapter;
mod grep_adapter;
mod gzip_adapter;
mod ios_commands;
mod ios_host;
mod jq_adapter;
mod mosh_adapter;
#[cfg(feature = "node")]
mod node_adapter;
#[cfg(feature = "vision")]
mod ocr_adapter;
#[cfg(feature = "python")]
mod python_adapter;
mod ripgrep_adapter;
mod sed_adapter;
pub mod selftest;
mod sftp_adapter;
mod sqlite_adapter;
mod ssh_adapter;
mod tar_adapter;
mod unzip_adapter;
mod uutils_adapter;
mod wget_adapter;

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use brush_builtins::BuiltinSet;
use brush_core::builtins as core_builtins;
use brush_core::extensions::DefaultShellExtensions;
use brush_core::openfiles::OpenFile;

pub type OutputCb = extern "C" fn(ctx: *mut c_void, bytes: *const u8, len: usize);
pub type DoneCb = extern "C" fn(ctx: *mut c_void, exit_code: i32, cwd: *const c_char);

/// Opaque Swift-side context pointer handed to the C callbacks.
///
/// SAFETY: the pointer is an `Unmanaged<ShellSession>` from Swift. It is only
/// dereferenced by Swift inside the callbacks; Rust just carries it between
/// threads. It stays valid because `ashell_session_free` joins both worker
/// threads before returning, so no callback fires after the Swift object is
/// released. `alive` is checked before every callback as belt-and-suspenders.
struct CallbackCtx(*mut c_void);
// SAFETY: the pointer is only dereferenced by Swift inside the callbacks; Rust
// merely carries it across threads. It stays valid because `ashell_session_free`
// joins both worker threads before returning (see the struct doc above).
unsafe impl Send for CallbackCtx {}

/// Runs an FFI body under `catch_unwind`, returning `default` on a panic so a
/// panic never unwinds across the C ABI (which is undefined behavior).
fn ffi_guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

/// Messages to the session's shell thread.
enum SessionMsg {
    Exec {
        id: u64,
        cmd: String,
        stdin: std::io::PipeReader,
    },
    /// Run a command and capture its output separately, replying with
    /// (exit, stdout, stderr). Used by non-interactive/agent callers.
    /// (command, optional timeout ms, reply channel).
    Capture(String, Option<u64>, mpsc::SyncSender<CaptureReply>),
    /// Tab completion: (line, cursor byte offset, reply channel).
    Complete(String, usize, mpsc::SyncSender<CompletionReply>),
}

/// Shell completion is delivered by the output-pump thread, never directly by
/// the shell thread. This creates one total order:
/// command output -> completion -> next prompt/input owner.
enum OutputEvent {
    Done(i32, String),
}

/// Captured output of one command (for `ashell_run_capture`).
pub struct CaptureReply {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// Completion result handed back to the FFI caller.
pub struct CompletionReply {
    /// Byte offset in the line where candidates are inserted.
    insertion_index: usize,
    /// Bytes to delete before insertion (the token prefix).
    delete_count: usize,
    candidates: Vec<String>,
}

pub struct Session {
    cmd_tx: mpsc::Sender<SessionMsg>,
    /// Writer for the command that currently owns terminal input. Each command
    /// gets a fresh pipe so unread Python/read bytes can never leak into the
    /// next shell command. Taking this value produces a real EOF.
    stdin_writer: Arc<Mutex<Option<(u64, std::io::PipeWriter)>>>,
    next_exec_id: AtomicU64,
    /// Cleared on free so lingering worker threads suppress their callbacks.
    alive: Arc<AtomicBool>,
    /// Signalled by `ashell_cancel` to interrupt the command currently running
    /// on the shell thread (best-effort — see `ashell_cancel`).
    cancel: Arc<tokio::sync::Notify>,
    reader_handle: Option<JoinHandle<()>>,
    shell_handle: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandSource {
    BrushBuiltin,
    UutilsCoreutils,
    Adapter,
    Runtime,
    Host,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub source: CommandSource,
}

/// Authoritative inventory used by coverage tests and documentation. Keeping
/// this derived from the same upstream registries as `build_shell` makes a new
/// Brush/uutils command fail the test-matrix gate until it has a contract test.
pub fn command_inventory() -> Vec<CommandInfo> {
    let mut commands = Vec::new();
    let mut brush =
        brush_builtins::default_builtins::<DefaultShellExtensions>(BuiltinSet::BashMode);
    for unsupported in ["disown", "logout", "exec", "suspend"] {
        brush.remove(unsupported);
    }
    commands.extend(brush.keys().map(|name| CommandInfo {
        name: name.clone(),
        source: CommandSource::BrushBuiltin,
    }));
    commands.extend(
        uutils_adapter::command_names()
            .into_iter()
            .map(|name| CommandInfo {
                name,
                source: CommandSource::UutilsCoreutils,
            }),
    );
    for name in [
        "grep", "egrep", "fgrep", "stat", "rg", "clear", "which", "find", "xargs", "tree", "diff",
        "cmp", "gzip", "gunzip", "sed", "curl", "wget", "tar", "zip", "unzip", "sqlite3", "jq",
        "git", "awk", "ssh", "scp", "sftp", "mosh", "edit", "vi", "nano",
    ] {
        commands.push(CommandInfo {
            name: name.to_string(),
            source: CommandSource::Adapter,
        });
    }
    for name in ["pbcopy", "pbpaste", "open", "openurl"] {
        commands.push(CommandInfo {
            name: name.to_string(),
            source: CommandSource::Host,
        });
    }
    #[cfg(feature = "python")]
    for name in ["python3", "python", "pip", "pip3"] {
        commands.push(CommandInfo {
            name: name.to_string(),
            source: CommandSource::Runtime,
        });
    }
    #[cfg(feature = "vision")]
    commands.push(CommandInfo {
        name: "ocr".to_string(),
        source: CommandSource::Host,
    });
    #[cfg(feature = "node")]
    for name in ["node", "npm", "npx"] {
        commands.push(CommandInfo {
            name: name.to_string(),
            source: CommandSource::Runtime,
        });
    }
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands.dedup_by(|a, b| a.name == b.name);
    commands
}

/// Builds a Shell configured identically for FFI sessions and the selftest
/// battery: bash-mode default builtins, the full uutils coreutils set (via
/// the fd-redirecting adapter), and library-backed commands.
pub(crate) async fn build_shell(
    fds: HashMap<brush_core::ShellFd, OpenFile>,
    working_dir: &std::path::Path,
) -> Result<brush_core::Shell, brush_core::Error> {
    let mut bash_builtins =
        brush_builtins::default_builtins::<DefaultShellExtensions>(BuiltinSet::BashMode);
    // These Unix process/job-control commands are either explicitly
    // unimplemented by Brush or fundamentally wrong for an iOS app host.
    // Remove them from the registry entirely so lookup, help and completion do
    // not advertise functionality that YourShell cannot safely provide.
    for unsupported in ["disown", "logout", "exec", "suspend"] {
        bash_builtins.remove(unsupported);
    }

    let mut builder = brush_core::Shell::builder()
        .builtins(bash_builtins)
        .builtin("grep", grep_adapter::registration())
        .builtin("egrep", grep_adapter::registration())
        .builtin("fgrep", grep_adapter::registration())
        .builtin("stat", uutils_adapter::stat_registration())
        .builtin("pbcopy", ios_commands::copy_registration())
        .builtin("pbpaste", ios_commands::paste_registration())
        .builtin("open", ios_commands::open_registration())
        .builtin("openurl", ios_commands::open_registration())
        // `rg` ships next to `grep`, never as an alias for it: rg recurses by
        // default and honours .gitignore, so aliasing would silently drop
        // matches. Both share ripgrep's engine; only the CLI semantics differ.
        .builtin("rg", ripgrep_adapter::registration())
        .builtin(
            "clear",
            core_builtins::builtin::<builtins_extra::ClearCommand, DefaultShellExtensions>(),
        )
        .builtin(
            "which",
            core_builtins::builtin::<commands_ext::WhichCommand, DefaultShellExtensions>(),
        )
        .builtin("find", findutils_adapter::find_registration())
        .builtin("xargs", findutils_adapter::xargs_registration())
        .builtin(
            "tree",
            core_builtins::builtin::<commands_ext::TreeCommand, DefaultShellExtensions>(),
        )
        .builtin("diff", diffutils_adapter::diff_registration())
        .builtin("cmp", diffutils_adapter::cmp_registration())
        .builtin("gzip", gzip_adapter::registration())
        .builtin("gunzip", gzip_adapter::registration())
        .builtin("sed", sed_adapter::registration())
        .builtin("curl", curl_adapter::registration())
        .builtin("wget", wget_adapter::registration())
        .builtin("tar", tar_adapter::registration())
        .builtin(
            "zip",
            core_builtins::builtin::<commands_ext::ZipCommand, DefaultShellExtensions>(),
        )
        .builtin("unzip", unzip_adapter::registration())
        .builtin("sqlite3", sqlite_adapter::registration())
        .builtin("jq", jq_adapter::registration())
        .builtin("git", git_adapter::registration())
        .builtin("awk", awk_adapter::registration())
        .builtin("ssh", ssh_adapter::registration())
        .builtin("scp", sftp_adapter::registration_scp())
        .builtin("sftp", sftp_adapter::registration_sftp())
        .builtin("mosh", mosh_adapter::registration())
        .builtin(
            "edit",
            core_builtins::builtin::<editor::EditorCommand, DefaultShellExtensions>(),
        )
        .builtin(
            "vi",
            core_builtins::builtin::<editor::EditorCommand, DefaultShellExtensions>(),
        )
        .builtin(
            "nano",
            core_builtins::builtin::<editor::EditorCommand, DefaultShellExtensions>(),
        );
    for name in uutils_adapter::command_names() {
        builder = builder.builtin(name, uutils_adapter::registration());
    }
    #[cfg(feature = "python")]
    {
        builder = builder
            .builtin("python3", python_adapter::registration())
            .builtin("python", python_adapter::registration())
            .builtin("pip", python_adapter::registration())
            .builtin("pip3", python_adapter::registration());
    }
    #[cfg(feature = "vision")]
    {
        builder = builder.builtin("ocr", ocr_adapter::registration());
    }
    #[cfg(feature = "node")]
    {
        builder = builder
            .builtin("node", node_adapter::registration())
            .builtin("npm", node_adapter::registration())
            .builtin("npx", node_adapter::registration());
    }
    builder
        .fds(fds)
        .working_dir(working_dir.to_path_buf())
        .shell_name("ashell".to_string())
        .build()
        .await
}

/// Installs app-owned capabilities for iOS Host commands. Each callback is
/// optional so the core remains usable in tests and non-UIKit hosts.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_ios_host_install(
    copy: Option<ios_host::CopyFn>,
    paste: Option<ios_host::PasteFn>,
    open: Option<ios_host::OpenFn>,
) -> i32 {
    ios_host::install(copy, paste, open) as i32
}

/// Builds the standard shell with no preset fds; used by the integration
/// tests (battery, concurrency) that manage fds per invocation themselves.
pub async fn build_shell_for_tests(
    working_dir: &std::path::Path,
) -> Result<brush_core::Shell, brush_core::Error> {
    build_shell(HashMap::new(), working_dir).await
}

/// Runs one command capturing stdout/stderr separately, with an optional
/// timeout and cooperative cancellation. Used by the non-interactive/agent
/// path (`ashell_run_capture`).
async fn run_capture(
    shell: &mut brush_core::Shell,
    cmd: String,
    timeout_ms: Option<u64>,
    cancel: &tokio::sync::Notify,
) -> CaptureReply {
    let (Ok((out_r, out_w)), Ok((err_r, err_w))) = (std::io::pipe(), std::io::pipe()) else {
        return CaptureReply {
            exit_code: 127,
            stdout: String::new(),
            stderr: "ashell: pipe allocation failed\n".to_string(),
        };
    };
    // Drain both pipes on their own threads so a command producing more than a
    // pipe buffer (~64 KB) can't deadlock while we're still awaiting run_string.
    let out_h = std::thread::spawn(move || {
        let mut b = Vec::new();
        let mut r = out_r;
        let _ = r.read_to_end(&mut b);
        b
    });
    let err_h = std::thread::spawn(move || {
        let mut b = Vec::new();
        let mut r = err_r;
        let _ = r.read_to_end(&mut b);
        b
    });

    // Redirect the SHELL's fd 0/1/2 to the capture pipes (not per-command
    // `params`, which the uutils adapter doesn't see — it resolves fds from the
    // shell's open_files). Save the originals to restore afterward so the
    // interactive path keeps working. Commands run one at a time on this thread,
    // so this temporary redirection is race-free.
    let nul = brush_core::openfiles::null().ok();
    let old0 = nul.map(|n| shell.open_files_mut().set_fd(0.into(), n));
    let old1 = shell
        .open_files_mut()
        .set_fd(1.into(), OpenFile::from(out_w));
    let old2 = shell
        .open_files_mut()
        .set_fd(2.into(), OpenFile::from(err_w));

    let params = shell.default_exec_params();
    let source_info = brush_core::SourceInfo::from("agent");
    let exit_code: i32 = tokio::select! {
        r = shell.run_string(cmd, &source_info, &params) => match r {
            Ok(result) => i32::from(u8::from(result.exit_code)),
            Err(_) => 127,
        },
        () = cancel.notified() => 130,      // SIGINT-equivalent
        () = async {
            match timeout_ms {
                Some(ms) => tokio::time::sleep(std::time::Duration::from_millis(ms)).await,
                None => std::future::pending::<()>().await,
            }
        } => 124,                           // timeout, like timeout(1)
    };
    drop(params);

    // Restore the session's original fds — this also drops the capture write
    // ends, so the drain threads see EOF.
    if let Some(Some(f)) = old0 {
        shell.open_files_mut().set_fd(0.into(), f);
    }
    if let Some(f) = old1 {
        shell.open_files_mut().set_fd(1.into(), f);
    }
    if let Some(f) = old2 {
        shell.open_files_mut().set_fd(2.into(), f);
    }

    if exit_code == 124 || exit_code == 130 {
        // Cancelled/timed out: run_string was dropped mid-flight, so a detached
        // spawn_blocking command may still hold a pipe write end — joining the
        // drain threads could block forever. Leave them to finish in the
        // background and return without the (now-moot) output.
        let reason = if exit_code == 124 {
            "timed out"
        } else {
            "cancelled"
        };
        CaptureReply {
            exit_code,
            stdout: String::new(),
            stderr: format!("ashell: command {reason}\n"),
        }
    } else {
        let stdout = String::from_utf8_lossy(&out_h.join().unwrap_or_default()).into_owned();
        let stderr = String::from_utf8_lossy(&err_h.join().unwrap_or_default()).into_owned();
        CaptureReply {
            exit_code,
            stdout,
            stderr,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_session_new(
    out_cb: OutputCb,
    done_cb: DoneCb,
    ctx: *mut c_void,
    working_dir: *const c_char,
) -> *mut Session {
    ffi_guard(std::ptr::null_mut(), || {
        if working_dir.is_null() {
            return std::ptr::null_mut();
        }
        // As a staticlib in a Swift app there is no Rust `main`, so the std
        // runtime's default SIGPIPE→SIG_IGN never runs. Without this, a command
        // that writes to a pipe whose reader has gone away (a cancelled capture,
        // a `head`-truncated pipeline) kills the whole app. Ignore it so writes
        // return EPIPE instead. Installed once, process-wide.
        static SIGPIPE_ONCE: std::sync::Once = std::sync::Once::new();
        SIGPIPE_ONCE.call_once(|| unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        });
        // SAFETY: null-checked above; the caller contracts to pass a valid
        // NUL-terminated C string (from Swift's `path` argument).
        let working_dir = unsafe { CStr::from_ptr(working_dir) }
            .to_string_lossy()
            .into_owned();

        // Resource exhaustion (fd limit) must not abort the app: return null so
        // Swift can surface the failure.
        let Ok((stdout_reader, stdout_writer)) = std::io::pipe() else {
            return std::ptr::null_mut();
        };
        let (cmd_tx, cmd_rx) = mpsc::channel::<SessionMsg>();
        let (output_event_tx, output_event_rx) = mpsc::channel::<OutputEvent>();
        let stdin_writer = Arc::new(Mutex::new(None));
        let stdin_writer_for_shell = stdin_writer.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let cancel = Arc::new(tokio::sync::Notify::new());
        let shell_cancel = cancel.clone();

        // Pump command output back to Swift.
        let out_ctx = CallbackCtx(ctx);
        let reader_alive = alive.clone();
        let reader_handle = std::thread::spawn(move || {
            let out_ctx = out_ctx;
            let mut reader = stdout_reader;
            let mut buf = [0u8; 8192];
            // The shell keeps stdout open for the whole session, so a blocking
            // read cannot also observe completion events. Nonblocking draining
            // lets this one thread serialize bytes and Done callbacks.
            let fd = reader.as_raw_fd();
            // SAFETY: fd belongs to `reader` and remains valid for this thread.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags >= 0 {
                // SAFETY: same valid fd; preserving all existing status flags.
                unsafe {
                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                }
            }
            let drain = |reader: &mut std::io::PipeReader, buf: &mut [u8; 8192]| -> bool {
                loop {
                    match reader.read(buf) {
                        Ok(0) => return false,
                        Ok(n) => {
                            if reader_alive.load(Ordering::Acquire) {
                                out_cb(out_ctx.0, buf.as_ptr(), n);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
                        Err(_) => return false,
                    }
                }
            };
            loop {
                if !drain(&mut reader, &mut buf) {
                    break;
                }
                match output_event_rx.recv_timeout(std::time::Duration::from_millis(2)) {
                    Ok(OutputEvent::Done(exit_code, cwd)) => {
                        // The event is sent only after the command has flushed
                        // its writers. Drain once more so every preceding byte
                        // reaches Swift before ownership returns to the prompt.
                        if !drain(&mut reader, &mut buf) {
                            break;
                        }
                        let cwd = CString::new(cwd).unwrap_or_default();
                        if reader_alive.load(Ordering::Acquire) {
                            done_cb(out_ctx.0, exit_code, cwd.as_ptr());
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        if !drain(&mut reader, &mut buf) {
                            break;
                        }
                        break;
                    }
                }
            }
        });

        // Shell thread: owns the Shell instance for this session's lifetime.
        let shell_handle = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };

            runtime.block_on(async move {
                let mut fds: HashMap<brush_core::ShellFd, OpenFile> = HashMap::new();
                if let Ok(nul) = brush_core::openfiles::null() {
                    fds.insert(0.into(), nul);
                }
                let Ok(stdout_writer2) = stdout_writer.try_clone() else {
                    return;
                };
                fds.insert(1.into(), OpenFile::from(stdout_writer2));
                fds.insert(2.into(), OpenFile::from(stdout_writer));

                let mut shell = match build_shell(fds, std::path::Path::new(&working_dir)).await {
                    Ok(s) => s,
                    Err(e) => {
                        let msg =
                            CString::new(format!("shell init failed: {e}")).unwrap_or_default();
                        let _ = output_event_tx
                            .send(OutputEvent::Done(127, msg.to_string_lossy().into_owned()));
                        return;
                    }
                };

                while let Ok(msg) = cmd_rx.recv() {
                    match msg {
                        SessionMsg::Exec { id, cmd, stdin } => {
                            let old_stdin = shell
                                .open_files_mut()
                                .set_fd(0.into(), OpenFile::from(stdin));
                            let params = shell.default_exec_params();
                            let source_info = brush_core::SourceInfo::from("terminal");
                            let exit_code: i32 = tokio::select! {
                                r = shell.run_string(cmd, &source_info, &params) => match r {
                                    Ok(result) => i32::from(u8::from(result.exit_code)),
                                    Err(e) => {
                                        let mut err = params.stderr(&shell);
                                        let _ = writeln!(err, "ashell: {e}");
                                        let _ = err.flush();
                                        127
                                    }
                                },
                                // Cancelled: the run_string future is dropped,
                                // aborting async awaits (ssh/curl/…). 130 = SIGINT.
                                () = shell_cancel.notified() => 130,
                            };
                            drop(params);
                            if let Some(old) = old_stdin {
                                shell.open_files_mut().set_fd(0.into(), old);
                            }
                            if let Ok(mut current) = stdin_writer_for_shell.lock() {
                                if current.as_ref().is_some_and(|(active, _)| *active == id) {
                                    current.take();
                                }
                            }
                            let cwd = shell.working_dir().to_string_lossy().into_owned();
                            let _ = output_event_tx.send(OutputEvent::Done(exit_code, cwd));
                        }
                        SessionMsg::Capture(cmd, timeout_ms, reply) => {
                            let out = run_capture(&mut shell, cmd, timeout_ms, &shell_cancel).await;
                            let _ = reply.send(out);
                        }
                        SessionMsg::Complete(line, pos, reply) => {
                            // Config is cloned to avoid borrowing shell immutably
                            // and mutably at once.
                            let cfg = shell.completion_config().clone();
                            let result = cfg.get_completions(&mut shell, &line, pos).await;
                            let out = match result {
                                Ok(c) => CompletionReply {
                                    insertion_index: c.insertion_index,
                                    delete_count: c.delete_count,
                                    candidates: c.candidates,
                                },
                                Err(_) => CompletionReply {
                                    insertion_index: pos,
                                    delete_count: 0,
                                    candidates: Vec::new(),
                                },
                            };
                            let _ = reply.send(out);
                        }
                    }
                }
            });
        });

        Box::into_raw(Box::new(Session {
            cmd_tx,
            stdin_writer,
            next_exec_id: AtomicU64::new(1),
            alive,
            cancel,
            reader_handle: Some(reader_handle),
            shell_handle: Some(shell_handle),
        }))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_exec(session: *mut Session, cmd: *const c_char) {
    if session.is_null() || cmd.is_null() {
        return;
    }
    ffi_guard((), || {
        // SAFETY: both pointers null-checked above; `session` is a live handle
        // from ashell_session_new and `cmd` a valid C string, per the contract.
        let session = unsafe { &*session };
        let cmd = unsafe { CStr::from_ptr(cmd) }
            .to_string_lossy()
            .into_owned();
        let Ok((stdin, writer)) = std::io::pipe() else {
            return;
        };
        let id = session.next_exec_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut current) = session.stdin_writer.lock() {
            *current = Some((id, writer));
        } else {
            return;
        }
        if session
            .cmd_tx
            .send(SessionMsg::Exec { id, cmd, stdin })
            .is_err()
        {
            if let Ok(mut current) = session.stdin_writer.lock() {
                current.take();
            }
        }
    });
}

/// Captured result of `ashell_run_capture`. Free with `ashell_capture_free`.
#[repr(C)]
pub struct CaptureResult {
    pub exit_code: i32,
    pub stdout: *mut c_char,
    pub stderr: *mut c_char,
}

/// Runs `cmd` to completion and returns its exit code with stdout/stderr
/// captured SEPARATELY (unlike the interactive `ashell_exec`, whose output is
/// merged). Blocks the calling thread until the command finishes, is cancelled
/// (`ashell_cancel`), or hits `timeout_ms` (0 = no timeout). For the agent /
/// non-interactive path. Returns null on error; free the result with
/// `ashell_capture_free`.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_run_capture(
    session: *mut Session,
    cmd: *const c_char,
    timeout_ms: u64,
) -> *mut CaptureResult {
    if session.is_null() || cmd.is_null() {
        return std::ptr::null_mut();
    }
    ffi_guard(std::ptr::null_mut(), || {
        // SAFETY: both pointers null-checked above; valid per the FFI contract.
        let session = unsafe { &*session };
        let cmd = unsafe { CStr::from_ptr(cmd) }
            .to_string_lossy()
            .into_owned();
        let (tx, rx) = mpsc::sync_channel::<CaptureReply>(1);
        let timeout = (timeout_ms != 0).then_some(timeout_ms);
        if session
            .cmd_tx
            .send(SessionMsg::Capture(cmd, timeout, tx))
            .is_err()
        {
            return std::ptr::null_mut();
        }
        let Ok(reply) = rx.recv() else {
            return std::ptr::null_mut();
        };
        // A C string can't carry interior NULs; strip them (agent output is
        // text — binary handling is the caller's policy).
        let to_c = |s: String| {
            CString::new(s.replace('\0', ""))
                .unwrap_or_default()
                .into_raw()
        };
        Box::into_raw(Box::new(CaptureResult {
            exit_code: reply.exit_code,
            stdout: to_c(reply.stdout),
            stderr: to_c(reply.stderr),
        }))
    })
}

/// Requests cancellation of the command currently running on the session's
/// shell thread. Best-effort: async awaits (ssh/mosh/network) are dropped at
/// their `.await` point; a command executing vendored C in a blocking thread
/// (uutils/python/node) can't be force-stopped and finishes in the background
/// while its result is discarded. A no-op when nothing is running.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_cancel(session: *mut Session) {
    if session.is_null() {
        return;
    }
    ffi_guard((), || {
        // SAFETY: null-checked above; valid per the FFI contract.
        let session = unsafe { &*session };
        // notify_waiters (not notify_one) so a cancel with no command running
        // doesn't leave a permit that would spuriously cancel the next command.
        session.cancel.notify_waiters();
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_capture_free(result: *mut CaptureResult) {
    if result.is_null() {
        return;
    }
    ffi_guard((), || {
        // SAFETY: reclaims a CaptureResult from ashell_run_capture; call once.
        let r = unsafe { Box::from_raw(result) };
        if !r.stdout.is_null() {
            drop(unsafe { CString::from_raw(r.stdout) });
        }
        if !r.stderr.is_null() {
            drop(unsafe { CString::from_raw(r.stderr) });
        }
    });
}

/// Tab completion. Given the current line and cursor byte offset, returns the
/// candidates as newline-joined text; the first line is a header
/// `<insertion_index> <delete_count> <count>` so the caller can apply them.
/// Caller frees with `ashell_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_complete(
    session: *mut Session,
    line: *const c_char,
    cursor: usize,
) -> *mut c_char {
    let empty = || CString::new("0 0 0").unwrap_or_default().into_raw();
    if session.is_null() || line.is_null() {
        return empty();
    }
    ffi_guard(empty(), || {
        // SAFETY: both pointers null-checked above; valid per the FFI contract.
        let session = unsafe { &*session };
        let line = unsafe { CStr::from_ptr(line) }
            .to_string_lossy()
            .into_owned();
        let (reply_tx, reply_rx) = mpsc::sync_channel::<CompletionReply>(1);
        if session
            .cmd_tx
            .send(SessionMsg::Complete(line, cursor, reply_tx))
            .is_err()
        {
            return empty();
        }
        let reply = match reply_rx.recv() {
            Ok(r) => r,
            Err(_) => return empty(),
        };
        let mut s = format!(
            "{} {} {}",
            reply.insertion_index,
            reply.delete_count,
            reply.candidates.len()
        );
        for c in &reply.candidates {
            s.push('\n');
            s.push_str(c);
        }
        CString::new(s).unwrap_or_default().into_raw()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_stdin_write(session: *mut Session, bytes: *const u8, len: usize) {
    if session.is_null() || (bytes.is_null() && len != 0) {
        return;
    }
    ffi_guard((), || {
        // SAFETY: `session` null-checked above and valid per the contract.
        // `&*session` (shared, not `&mut`) avoids aliasing `&mut Session` with
        // the `&*session` in ashell_exec; `PipeWriter` implements `Write` for
        // `&PipeWriter`.
        let session = unsafe { &*session };
        let data = if len == 0 {
            &[][..]
        } else {
            // SAFETY: bytes is non-null here (checked with len above); the
            // caller contracts that it points to `len` valid bytes.
            unsafe { std::slice::from_raw_parts(bytes, len) }
        };
        if let Ok(mut current) = session.stdin_writer.lock() {
            if let Some((_, writer)) = current.as_mut() {
                let _ = writer.write_all(data);
                let _ = writer.flush();
            }
        }
    });
}

/// Delivers terminal EOF to the command that currently owns stdin. Unlike an
/// EOT byte (`0x04`), dropping the command-scoped pipe is observed by
/// `read()`/Python's `input()` as a real EOF. The next command receives a fresh
/// pipe, so this cannot poison the session.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_stdin_eof(session: *mut Session) {
    if session.is_null() {
        return;
    }
    ffi_guard((), || {
        let session = unsafe { &*session };
        if let Ok(mut current) = session.stdin_writer.lock() {
            current.take();
        }
    });
}

/// Frees a session, joining its worker threads first so no callback can fire
/// against a released Swift context afterward. Idempotent-safe against null.
///
/// Note: if a command is still running when a session is freed (e.g. a hung
/// network command in `block_on`), the shell thread only exits after that
/// command returns, so this can block. Sessions are normally long-lived and
/// freed at teardown, so this is acceptable.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_session_free(session: *mut Session) {
    if session.is_null() {
        return;
    }
    ffi_guard((), || {
        // SAFETY: null-checked above; reclaims the Box leaked by
        // ashell_session_new. The caller must not free the same handle twice.
        let mut boxed = unsafe { Box::from_raw(session) };
        // Suppress any further callbacks, then signal both workers to stop.
        boxed.alive.store(false, Ordering::Release);
        let shell_handle = boxed.shell_handle.take();
        let reader_handle = boxed.reader_handle.take();
        // Drop the command channel + stdin so the shell thread's recv() ends and
        // any command blocked on stdin sees EOF; the shell thread then drops the
        // stdout writer, giving the reader thread EOF.
        let Session {
            cmd_tx,
            stdin_writer,
            ..
        } = *boxed;
        drop(cmd_tx);
        drop(stdin_writer);
        if let Some(h) = shell_handle {
            let _ = h.join();
        }
        if let Some(h) = reader_handle {
            let _ = h.join();
        }
    });
}

/// Runs the full test battery in-process; returns a heap-allocated report
/// string the caller must free with `ashell_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_selftest(working_dir: *const c_char) -> *mut c_char {
    if working_dir.is_null() {
        return std::ptr::null_mut();
    }
    ffi_guard(std::ptr::null_mut(), || {
        // SAFETY: null-checked above; a valid C string per the contract.
        let working_dir = unsafe { CStr::from_ptr(working_dir) }
            .to_string_lossy()
            .into_owned();
        let report = selftest::run_selftest(std::path::Path::new(&working_dir));
        CString::new(report).unwrap_or_default().into_raw()
    })
}

#[cfg(test)]
mod interactive_session_tests {
    use super::*;

    #[derive(Debug)]
    enum Event {
        Output(Vec<u8>),
        Done(i32),
    }

    struct Harness {
        tx: mpsc::Sender<Event>,
    }

    extern "C" fn output(ctx: *mut c_void, bytes: *const u8, len: usize) {
        // SAFETY: the test keeps Harness alive until the session is freed.
        let harness = unsafe { &*(ctx.cast::<Harness>()) };
        // SAFETY: output callback contract guarantees `len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
        let _ = harness.tx.send(Event::Output(bytes.to_vec()));
    }

    extern "C" fn done(ctx: *mut c_void, exit_code: i32, _cwd: *const c_char) {
        // SAFETY: the test keeps Harness alive until the session is freed.
        let harness = unsafe { &*(ctx.cast::<Harness>()) };
        let _ = harness.tx.send(Event::Done(exit_code));
    }

    fn exec(session: *mut Session, command: &str) {
        let command = CString::new(command).unwrap();
        ashell_exec(session, command.as_ptr());
    }

    fn write_stdin(session: *mut Session, bytes: &[u8]) {
        ashell_stdin_write(session, bytes.as_ptr(), bytes.len());
    }

    #[test]
    fn completion_follows_all_command_output() {
        let (tx, rx) = mpsc::channel();
        let harness = Box::new(Harness { tx });
        let cwd = CString::new(std::env::temp_dir().to_string_lossy().as_bytes()).unwrap();
        let session = ashell_session_new(
            output,
            done,
            (&*harness as *const Harness).cast_mut().cast(),
            cwd.as_ptr(),
        );
        assert!(!session.is_null());

        exec(session, "printf sentinel");
        let mut output_before_done = Vec::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
                Event::Output(bytes) => output_before_done.extend(bytes),
                Event::Done(code) => {
                    assert_eq!(code, 0);
                    break;
                }
            }
        }
        assert_eq!(output_before_done, b"sentinel");
        ashell_session_free(session);
    }

    #[test]
    fn unread_stdin_cannot_leak_into_the_next_command() {
        let (tx, rx) = mpsc::channel();
        let harness = Box::new(Harness { tx });
        let cwd = CString::new(std::env::temp_dir().to_string_lossy().as_bytes()).unwrap();
        let session = ashell_session_new(
            output,
            done,
            (&*harness as *const Harness).cast_mut().cast(),
            cwd.as_ptr(),
        );
        assert!(!session.is_null());

        exec(session, "read first; printf 'first:%s\\n' \"$first\"");
        write_stdin(session, b"abc\nSHOULD_NOT_LEAK\n");
        while !matches!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            Event::Done(0)
        ) {}

        exec(session, "cat");
        ashell_stdin_eof(session);
        let mut second_output = Vec::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
                Event::Output(bytes) => second_output.extend(bytes),
                Event::Done(code) => {
                    assert_eq!(code, 0);
                    break;
                }
            }
        }
        assert!(
            !String::from_utf8_lossy(&second_output).contains("SHOULD_NOT_LEAK"),
            "leftover stdin crossed the command ownership boundary"
        );
        ashell_session_free(session);
    }
}

/// ABI version, so the Swift host can verify it matches the linked core.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: null-checked; reclaims a CString produced by into_raw in this
        // module (ashell_complete/ashell_selftest). Must be called at most once.
        drop(unsafe { CString::from_raw(s) });
    }
}

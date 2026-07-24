//! FFI surface for the iOS shell MVP.
//!
//! Each session owns a dedicated OS thread running a current-thread tokio
//! runtime with one `brush_core::Shell` instance. The shell's fd 0/1/2 are
//! wired to real pipes; a reader thread pumps combined stdout/stderr bytes to
//! a Swift callback. cwd/env live in the Shell instance, so multiple sessions
//! in one process stay isolated.

mod builtins_extra;
mod commands_ext;
mod uutils_adapter;
#[cfg(feature = "python")]
mod python_adapter;
#[cfg(feature = "vision")]
mod ocr_adapter;
#[cfg(feature = "node")]
mod node_adapter;
pub mod selftest;

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{Read, Write};
use std::sync::mpsc;

use brush_builtins::{BuiltinSet, ShellBuilderExt};
use brush_core::builtins as core_builtins;
use brush_core::extensions::DefaultShellExtensions;
use brush_core::openfiles::OpenFile;

pub type OutputCb = extern "C" fn(ctx: *mut c_void, bytes: *const u8, len: usize);
pub type DoneCb = extern "C" fn(ctx: *mut c_void, exit_code: i32, cwd: *const c_char);

struct CallbackCtx(*mut c_void);
unsafe impl Send for CallbackCtx {}

pub struct Session {
    cmd_tx: mpsc::Sender<String>,
    stdin_writer: std::io::PipeWriter,
}

/// Builds a Shell configured identically for FFI sessions and the selftest
/// battery: bash-mode default builtins, the full uutils coreutils set (via
/// the fd-redirecting adapter), and library-backed commands.
pub(crate) async fn build_shell(
    fds: HashMap<brush_core::ShellFd, OpenFile>,
    working_dir: &std::path::Path,
) -> Result<brush_core::Shell, brush_core::Error> {
    let mut builder = brush_core::Shell::builder()
        .default_builtins(BuiltinSet::BashMode)
        .builtin("grep", core_builtins::builtin::<builtins_extra::GrepCommand, DefaultShellExtensions>())
        .builtin("clear", core_builtins::builtin::<builtins_extra::ClearCommand, DefaultShellExtensions>())
        .builtin("which", core_builtins::builtin::<commands_ext::WhichCommand, DefaultShellExtensions>())
        .builtin("find", core_builtins::builtin::<commands_ext::FindCommand, DefaultShellExtensions>())
        .builtin("tree", core_builtins::builtin::<commands_ext::TreeCommand, DefaultShellExtensions>())
        .builtin("diff", core_builtins::builtin::<commands_ext::DiffCommand, DefaultShellExtensions>())
        .builtin("gzip", core_builtins::builtin::<commands_ext::GzipCommand, DefaultShellExtensions>())
        .builtin("gunzip", core_builtins::builtin::<commands_ext::GunzipCommand, DefaultShellExtensions>())
        .builtin("sed", core_builtins::builtin::<commands_ext::SedCommand, DefaultShellExtensions>())
        .builtin("curl", core_builtins::builtin::<commands_ext::CurlCommand, DefaultShellExtensions>())
        .builtin("wget", core_builtins::builtin::<commands_ext::WgetCommand, DefaultShellExtensions>())
        .builtin("tar", core_builtins::builtin::<commands_ext::TarCommand, DefaultShellExtensions>())
        .builtin("zip", core_builtins::builtin::<commands_ext::ZipCommand, DefaultShellExtensions>())
        .builtin("unzip", core_builtins::builtin::<commands_ext::UnzipCommand, DefaultShellExtensions>());
    for name in uutils_adapter::command_names() {
        builder = builder.builtin(name, uutils_adapter::registration());
    }
    #[cfg(feature = "python")]
    {
        builder = builder
            .builtin("python3", python_adapter::registration())
            .builtin("python", python_adapter::registration());
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

/// Builds the standard shell with no preset fds; used by the integration
/// tests (battery, concurrency) that manage fds per invocation themselves.
pub async fn build_shell_for_tests(
    working_dir: &std::path::Path,
) -> Result<brush_core::Shell, brush_core::Error> {
    build_shell(HashMap::new(), working_dir).await
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_session_new(
    out_cb: OutputCb,
    done_cb: DoneCb,
    ctx: *mut c_void,
    working_dir: *const c_char,
) -> *mut Session {
    let working_dir = unsafe { CStr::from_ptr(working_dir) }
        .to_string_lossy()
        .into_owned();

    let (stdout_reader, stdout_writer) = std::io::pipe().expect("pipe");
    let (stdin_reader, stdin_writer) = std::io::pipe().expect("pipe");
    let (cmd_tx, cmd_rx) = mpsc::channel::<String>();

    // Pump command output back to Swift.
    let out_ctx = CallbackCtx(ctx);
    std::thread::spawn(move || {
        let out_ctx = out_ctx;
        let mut reader = stdout_reader;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => out_cb(out_ctx.0, buf.as_ptr(), n),
            }
        }
    });

    // Shell thread: owns the Shell instance for this session's lifetime.
    let shell_ctx = CallbackCtx(ctx);
    std::thread::spawn(move || {
        let shell_ctx = shell_ctx;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        runtime.block_on(async move {
            let mut fds: HashMap<brush_core::ShellFd, OpenFile> = HashMap::new();
            fds.insert(0.into(), OpenFile::from(stdin_reader));
            fds.insert(1.into(), OpenFile::from(stdout_writer.try_clone().expect("clone")));
            fds.insert(2.into(), OpenFile::from(stdout_writer));

            let shell = build_shell(fds, std::path::Path::new(&working_dir)).await;

            let mut shell = match shell {
                Ok(s) => s,
                Err(e) => {
                    let msg = CString::new(format!("shell init failed: {e}")).unwrap();
                    done_cb(shell_ctx.0, 127, msg.as_ptr());
                    return;
                }
            };

            while let Ok(cmd) = cmd_rx.recv() {
                let params = shell.default_exec_params();
                let source_info = brush_core::SourceInfo::from("terminal");
                let exit_code: i32 = match shell.run_string(cmd, &source_info, &params).await {
                    Ok(result) => i32::from(u8::from(result.exit_code)),
                    Err(e) => {
                        let mut err = params.stderr(&shell);
                        let _ = writeln!(err, "ashell: {e}");
                        let _ = err.flush();
                        127
                    }
                };
                let cwd = CString::new(shell.working_dir().to_string_lossy().into_owned())
                    .unwrap_or_default();
                done_cb(shell_ctx.0, exit_code, cwd.as_ptr());
            }
        });
    });

    Box::into_raw(Box::new(Session { cmd_tx, stdin_writer }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_exec(session: *mut Session, cmd: *const c_char) {
    let session = unsafe { &*session };
    let cmd = unsafe { CStr::from_ptr(cmd) }.to_string_lossy().into_owned();
    let _ = session.cmd_tx.send(cmd);
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_stdin_write(session: *mut Session, bytes: *const u8, len: usize) {
    let session = unsafe { &mut *session };
    let data = unsafe { std::slice::from_raw_parts(bytes, len) };
    let _ = session.stdin_writer.write_all(data);
    let _ = session.stdin_writer.flush();
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_session_free(session: *mut Session) {
    drop(unsafe { Box::from_raw(session) });
}

/// Runs the full test battery in-process; returns a heap-allocated report
/// string the caller must free with `ashell_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn ashell_selftest(working_dir: *const c_char) -> *mut c_char {
    let working_dir = unsafe { CStr::from_ptr(working_dir) }
        .to_string_lossy()
        .into_owned();
    let report = selftest::run_selftest(std::path::Path::new(&working_dir));
    CString::new(report).unwrap_or_default().into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn ashell_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

//! Real localhost protocol tests for the in-process ssh/scp/sftp adapters.
//! The test launches an isolated OpenSSH daemon with throwaway keys and never
//! changes the machine's system sshd configuration.

use std::ffi::{c_char, c_void, CStr, CString};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ashellcore::{
    ashell_capture_free, ashell_run_capture, ashell_session_free, ashell_session_new,
    CaptureResult, Session,
};

extern "C" fn out_noop(_ctx: *mut c_void, _bytes: *const u8, _len: usize) {}
extern "C" fn done_noop(_ctx: *mut c_void, _code: i32, _cwd: *const c_char) {}

struct TestSshd {
    child: Child,
    root: PathBuf,
    port: u16,
    user: String,
    identity: PathBuf,
}

impl Drop for TestSshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn command_ok(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().unwrap();
    assert!(status.success(), "{program} {args:?} failed: {status}");
}

fn start_sshd() -> TestSshd {
    let root = std::env::temp_dir().join(format!("yourshell-sshd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let host_key = root.join("host_ed25519");
    let identity = root.join("client_ed25519");
    command_ok(
        "/usr/bin/ssh-keygen",
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            host_key.to_str().unwrap(),
        ],
    );
    command_ok(
        "/usr/bin/ssh-keygen",
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            identity.to_str().unwrap(),
        ],
    );

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let user = std::env::var("USER").expect("USER is required for localhost sshd test");
    let authorized_keys = identity.with_extension("pub");
    let child = Command::new("/usr/sbin/sshd")
        .args([
            "-D",
            "-e",
            "-p",
            &port.to_string(),
            "-h",
            host_key.to_str().unwrap(),
            "-o",
            &format!("AuthorizedKeysFile={}", authorized_keys.display()),
            "-o",
            "StrictModes=no",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "UsePAM=no",
            "-o",
            "PubkeyAuthentication=yes",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "Subsystem=sftp internal-sftp",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start isolated sshd");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return TestSshd {
                child,
                root,
                port,
                user,
                identity,
            };
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("isolated sshd did not listen on port {port}");
}

fn new_session(cwd: &Path) -> *mut Session {
    let cwd = CString::new(cwd.to_string_lossy().as_bytes()).unwrap();
    let session = ashell_session_new(out_noop, done_noop, std::ptr::null_mut(), cwd.as_ptr());
    assert!(!session.is_null());
    session
}

fn capture(session: *mut Session, command: &str) -> (i32, String, String) {
    let command = CString::new(command).unwrap();
    let result = ashell_run_capture(session, command.as_ptr(), 20_000);
    assert!(!result.is_null());
    let result_ref: &CaptureResult = unsafe { &*result };
    let stdout = unsafe { CStr::from_ptr(result_ref.stdout) }
        .to_string_lossy()
        .into_owned();
    let stderr = unsafe { CStr::from_ptr(result_ref.stderr) }
        .to_string_lossy()
        .into_owned();
    let code = result_ref.exit_code;
    ashell_capture_free(result);
    (code, stdout, stderr)
}

fn require_success(label: &str, result: &(i32, String, String)) {
    assert_eq!(
        result.0, 0,
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        result.1, result.2
    );
}

fn main() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let server = start_sshd();
    let session = new_session(&server.root);
    let setup = format!("export HOME={} USER={}", server.root.display(), server.user);
    require_success("configure session", &capture(session, &setup));
    let destination = format!("{}@127.0.0.1", server.user);

    let ssh = capture(
        session,
        &format!(
            "ssh -p {} -i {} {} printf YOURSHELL_SSH_OK",
            server.port,
            server.identity.display(),
            destination
        ),
    );
    require_success("ssh command", &ssh);
    assert_eq!(ssh.1, "YOURSHELL_SSH_OK");
    let ssh_trusted = capture(
        session,
        &format!(
            "ssh -p {} -i {} {} printf TRUSTED_RECONNECT",
            server.port,
            server.identity.display(),
            destination
        ),
    );
    require_success("ssh trusted reconnect", &ssh_trusted);
    assert_eq!(ssh_trusted.1, "TRUSTED_RECONNECT");

    let local = server.root.join("upload.txt");
    let remote = server.root.join("remote.txt");
    let downloaded = server.root.join("downloaded.txt");
    std::fs::write(&local, b"scp-real-protocol").unwrap();
    let scp_upload = capture(
        session,
        &format!(
            "scp -P {} -i {} {} {}:{}",
            server.port,
            server.identity.display(),
            local.display(),
            destination,
            remote.display()
        ),
    );
    require_success("scp upload", &scp_upload);
    assert_eq!(std::fs::read(&remote).unwrap(), b"scp-real-protocol");

    let scp_download = capture(
        session,
        &format!(
            "scp -P {} -i {} {}:{} {}",
            server.port,
            server.identity.display(),
            destination,
            remote.display(),
            downloaded.display()
        ),
    );
    require_success("scp download", &scp_download);
    assert_eq!(std::fs::read(&downloaded).unwrap(), b"scp-real-protocol");

    let source_dir = server.root.join("source-dir");
    let remote_dir = server.root.join("remote-dir");
    std::fs::create_dir(&source_dir).unwrap();
    std::fs::write(source_dir.join("nested.txt"), b"recursive-scp").unwrap();
    let scp_recursive = capture(
        session,
        &format!(
            "scp -r -P {} -i {} {} {}:{}",
            server.port,
            server.identity.display(),
            source_dir.display(),
            destination,
            remote_dir.display()
        ),
    );
    require_success("scp recursive upload", &scp_recursive);
    assert_eq!(
        std::fs::read(remote_dir.join("nested.txt")).unwrap(),
        b"recursive-scp"
    );

    let sftp_remote = server.root.join("sftp.txt");
    let sftp_download = server.root.join("sftp-downloaded.txt");
    let sftp_command = format!(
        "printf 'put {} {}\\nls {}\\nget {} {}\\nquit\\n' | sftp -P {} -i {} {}",
        local.display(),
        sftp_remote.display(),
        sftp_remote.display(),
        sftp_remote.display(),
        sftp_download.display(),
        server.port,
        server.identity.display(),
        destination
    );
    let sftp = capture(session, &sftp_command);
    require_success("sftp batch", &sftp);
    assert_eq!(std::fs::read(&sftp_remote).unwrap(), b"scp-real-protocol");
    assert_eq!(std::fs::read(&sftp_download).unwrap(), b"scp-real-protocol");

    ashell_session_free(session);
    println!("=== localhost ssh/scp/sftp real protocol passed ===");
}

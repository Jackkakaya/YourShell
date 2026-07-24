//! Throwaway SFTP server for verifying the `scp`/`sftp` builtins end-to-end —
//! NOT shipped, host-only. Serves a real directory (arg 2, default a fresh temp
//! dir) over the SFTP subsystem. Accepts password "testpw".
//!
//!   cargo run --release --example test_sftp_server -- 127.0.0.1:2223 /some/dir

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::keys::PrivateKey;
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle as SftpHandle, Name, OpenFlags, Status, StatusCode,
    Version,
};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACASUohKfMuQ2nSEjXhqyzuY9wqL0U7Q/bDByT/cJIhkwwAAAJAZNYikGTWI
pAAAAAtzc2gtZWQyNTUxOQAAACASUohKfMuQ2nSEjXhqyzuY9wqL0U7Q/bDByT/cJIhkww
AAAEAv8BDRB99pH38H1yBQMD/6JeyEcwAdp6tOHhhbzOp7JxJSiEp8y5DadISNeGrLO5j3
CovRTtD9sMHJP9wkiGTDAAAACXRocm93YXdheQECAwQ=
-----END OPENSSH PRIVATE KEY-----
";

#[derive(Clone)]
struct Srv {
    root: PathBuf,
}

impl server::Server for Srv {
    type Handler = SshSession;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> SshSession {
        SshSession {
            root: self.root.clone(),
            channel: Arc::new(Mutex::new(None)),
        }
    }
}

struct SshSession {
    root: PathBuf,
    channel: Arc<Mutex<Option<Channel<Msg>>>>,
}

impl server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, _u: &str, p: &str) -> Result<Auth, Self::Error> {
        if p == "testpw" {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject { proceed_with_methods: None, partial_success: false })
        }
    }

    async fn auth_publickey(
        &mut self,
        _u: &str,
        _k: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Reject { proceed_with_methods: None, partial_success: false })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        *self.channel.lock().await = Some(channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            let channel = self.channel.lock().await.take().expect("channel");
            session.channel_success(channel_id)?;
            let handler = Sftp::new(self.root.clone());
            russh_sftp::server::run(channel.into_stream(), handler).await;
        } else {
            session.channel_failure(channel_id)?;
        }
        Ok(())
    }
}

/// Filesystem-backed SFTP handler rooted at `root`. Uses the real absolute path
/// as the opaque handle for both files and directories.
struct Sftp {
    root: PathBuf,
    dir_done: HashSet<String>,
}

impl Sftp {
    fn new(root: PathBuf) -> Self {
        Self { root, dir_done: HashSet::new() }
    }

    /// Maps an SFTP path onto the served root, preventing escape via `..`.
    fn real(&self, p: &str) -> PathBuf {
        let rel = Path::new(p.trim_start_matches('/'));
        let mut out = self.root.clone();
        for c in rel.components() {
            match c {
                std::path::Component::Normal(s) => out.push(s),
                std::path::Component::ParentDir => {
                    out.pop();
                }
                _ => {}
            }
        }
        out
    }
}

fn ok_status(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _v: u32,
        _e: std::collections::HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        // Report a canonical absolute path under the virtual root.
        let canon = if path == "." || path.is_empty() { "/".to_string() } else { path };
        Ok(Name { id, files: vec![File::dummy(canon)] })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let md = std::fs::metadata(self.real(&path)).map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs { id, attrs: FileAttributes::from(&md) })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let md = std::fs::symlink_metadata(self.real(&path)).map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs { id, attrs: FileAttributes::from(&md) })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<SftpHandle, Self::Error> {
        let path = self.real(&filename);
        if pflags.contains(OpenFlags::WRITE) {
            // Create/truncate so subsequent writes land in a fresh file.
            std::fs::File::create(&path).map_err(|_| StatusCode::PermissionDenied)?;
        } else if !path.exists() {
            return Err(StatusCode::NoSuchFile);
        }
        let _ = id;
        Ok(SftpHandle { id, handle: filename })
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(self.real(&handle)).map_err(|_| StatusCode::NoSuchFile)?;
        f.seek(SeekFrom::Start(offset)).map_err(|_| StatusCode::Failure)?;
        let mut buf = vec![0u8; len as usize];
        let n = f.read(&mut buf).map_err(|_| StatusCode::Failure)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(self.real(&handle))
            .map_err(|_| StatusCode::PermissionDenied)?;
        f.seek(SeekFrom::Start(offset)).map_err(|_| StatusCode::Failure)?;
        f.write_all(&data).map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<SftpHandle, Self::Error> {
        self.dir_done.remove(&path);
        if !self.real(&path).is_dir() {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(SftpHandle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if self.dir_done.contains(&handle) {
            return Err(StatusCode::Eof);
        }
        self.dir_done.insert(handle.clone());
        let mut files = Vec::new();
        let rd = std::fs::read_dir(self.real(&handle)).map_err(|_| StatusCode::NoSuchFile)?;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let attrs = e.metadata().map(|m| FileAttributes::from(&m)).unwrap_or_default();
            files.push(File::new(name, attrs));
        }
        Ok(Name { id, files })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(ok_status(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        std::fs::create_dir_all(self.real(&path)).map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.real(&filename)).map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir_all(self.real(&path)).map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        std::fs::rename(self.real(&oldpath), self.real(&newpath)).map_err(|_| StatusCode::Failure)?;
        Ok(ok_status(id))
    }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:2223".into());
    let root = std::env::args()
        .nth(2)
        .map_or_else(|| std::env::temp_dir().join("yourshell_sftp_root"), PathBuf::from);
    std::fs::create_dir_all(&root).unwrap();
    let config = server::Config {
        keys: vec![PrivateKey::from_openssh(HOST_KEY).unwrap()],
        ..Default::default()
    };
    let listener = TcpListener::bind(&addr).await.unwrap();
    eprintln!("test_sftp_server on {addr}, root={} (password: testpw)", root.display());
    let mut srv = Srv { root };
    srv.run_on_socket(Arc::new(config), &listener).await.unwrap();
}

//! `scp` and `sftp` over the SFTP subsystem, using russh-sftp on top of the
//! same russh client connection as the `ssh` builtin (see [`crate::ssh_adapter`]).
//!
//! Both are file-transfer commands, so unlike `ssh` they are NOT full-screen:
//! `scp` runs and returns; `sftp` is a line-based REPL that reads commands off
//! the cooked-mode stdin. Authentication is key + `$SSH_PASSWORD` only (no
//! interactive prompt here). Each runs on a dedicated `enable_all` runtime
//! inside `spawn_blocking`, driving the session fds directly — no global state.

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

use brush_core::builtins::Registration;
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_fd::AsyncFd;

use crate::ssh_adapter::{connect_session, key_and_env_auth, HostKeyOutcome, Session};

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn registration_scp() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_scp,
        content_func: |name, _, _| Ok(format!("{name}: scp (russh-sftp, in-process)")),
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

pub fn registration_sftp() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_sftp,
        content_func: |name, _, _| Ok(format!("{name}: sftp (russh-sftp, in-process)")),
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

// ---------------------------------------------------------------------------
// Shared connection info
// ---------------------------------------------------------------------------

struct Conn {
    user: String,
    host: String,
    port: u16,
    identity: Option<PathBuf>,
    home: PathBuf,
    password_env: Option<String>,
}

fn env_reader<'a>(
    context: &'a ExecutionContext<'a, DefaultShellExtensions>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |k: &str| {
        context
            .shell
            .env()
            .get_str(k, context.shell)
            .map(|c| c.into_owned())
    }
}

/// Splits `[user@]host:path` into its parts. Returns None when `s` is a local
/// path (no `:` before the first `/`, matching scp's heuristic).
fn parse_remote(s: &str, default_user: &str) -> Option<(String, String, String)> {
    let colon = s.find(':')?;
    let slash = s.find('/');
    if slash.is_some_and(|sl| sl < colon) {
        return None; // "./a:b" style local path
    }
    let (hostpart, path) = s.split_at(colon);
    let path = &path[1..]; // drop ':'
    let (user, host) = match hostpart.split_once('@') {
        Some((u, h)) => (u.to_string(), h.to_string()),
        None => (default_user.to_string(), hostpart.to_string()),
    };
    let path = if path.is_empty() { ".".to_string() } else { path.to_string() };
    Some((user, host, path))
}

/// Uploads `data` to `dest`, waiting for the server to acknowledge every write
/// before returning. The high-level `SftpSession::write` sends writes
/// fire-and-forget (`write_nowait`) and does not flush, so a caller that tears
/// the connection down immediately can lose the data — `flush()` drains the
/// pending write-acks and `shutdown()` closes the handle.
async fn write_remote_file(sftp: &SftpSession, dest: &str, data: &[u8]) -> Result<(), String> {
    let mut f = sftp
        .open_with_flags(
            dest.to_string(),
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(|e| format!("open {dest}: {e}"))?;
    f.write_all(data).await.map_err(|e| format!("write {dest}: {e}"))?;
    f.flush().await.map_err(|e| format!("flush {dest}: {e}"))?;
    f.shutdown().await.map_err(|e| format!("close {dest}: {e}"))?;
    Ok(())
}

async fn open_sftp(session: &mut Session) -> Result<SftpSession, String> {
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("open channel: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("request sftp subsystem: {e}"))?;
    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("sftp init: {e}"))
}

async fn connect_and_open(conn: &Conn) -> Result<SftpSession, String> {
    let (res, hostkey) = connect_session(&conn.host, conn.port, &conn.home).await;
    let mut session = res.map_err(|e| {
        if hostkey == HostKeyOutcome::Changed {
            format!(
                "REMOTE HOST IDENTIFICATION HAS CHANGED for {} — possible MITM; \
                 remove the stale ~/.ssh/known_hosts line to override",
                conn.host
            )
        } else {
            format!("connect {}:{}: {e}", conn.host, conn.port)
        }
    })?;
    let authed = key_and_env_auth(
        &mut session,
        &conn.user,
        &conn.home,
        conn.identity.as_deref(),
        conn.password_env.as_deref(),
    )
    .await;
    if !authed {
        return Err(format!(
            "authentication failed for {}@{} (set SSH_PASSWORD or add a key)",
            conn.user, conn.host
        ));
    }
    open_sftp(&mut session).await
}

fn write_fd(fd: RawFd, data: &[u8]) {
    let mut data = data;
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n <= 0 {
            break;
        }
        data = &data[n as usize..];
    }
}

// ---------------------------------------------------------------------------
// scp
// ---------------------------------------------------------------------------

struct ScpOpts {
    port: u16,
    identity: Option<PathBuf>,
    recursive: bool,
    src: String,
    dst: String,
}

fn parse_scp(argv: &[String]) -> Result<ScpOpts, String> {
    let mut port = 22;
    let mut identity = None;
    let mut recursive = false;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "-r" => recursive = true,
            "-P" => {
                i += 1;
                port = argv
                    .get(i)
                    .and_then(|p| p.parse().ok())
                    .ok_or("scp: -P needs a port")?;
            }
            "-i" => {
                i += 1;
                identity = Some(PathBuf::from(argv.get(i).ok_or("scp: -i needs a path")?));
            }
            a if a.starts_with('-') => return Err(format!("scp: unsupported option {a}")),
            a => positional.push(a.to_string()),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return Err("usage: scp [-r] [-P port] [-i id] SRC DST".to_string());
    }
    Ok(ScpOpts {
        port,
        identity,
        recursive,
        src: positional[0].clone(),
        dst: positional[1].clone(),
    })
}

fn exec_scp(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();
        let getenv = env_reader(&context);
        let opts = match parse_scp(&argv) {
            Ok(o) => o,
            Err(m) => return fail(&context, &m, 2),
        };
        let default_user = getenv("USER").unwrap_or_else(|| "root".to_string());
        let home = getenv("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let password_env = getenv("SSH_PASSWORD");
        let cwd = context.shell.working_dir().to_path_buf();

        let src_remote = parse_remote(&opts.src, &default_user);
        let dst_remote = parse_remote(&opts.dst, &default_user);

        let fd1 = borrow_fd(&context, 1);
        let fd2 = borrow_fd(&context, 2);
        let (Some(fd1), Some(fd2)) = (fd1, fd2) else {
            return fail(&context, "scp: no output", 1);
        };

        let code = tokio::task::spawn_blocking(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(_) => return 255u8,
            };
            rt.block_on(run_scp(
                opts,
                src_remote,
                dst_remote,
                &cwd,
                &home,
                password_env,
                fd1.as_raw_fd(),
                fd2.as_raw_fd(),
            ))
        })
        .await
        .unwrap_or(255);

        Ok(ExecutionResult::new(code))
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_scp(
    opts: ScpOpts,
    src_remote: Option<(String, String, String)>,
    dst_remote: Option<(String, String, String)>,
    cwd: &Path,
    home: &Path,
    password_env: Option<String>,
    fd1: RawFd,
    fd2: RawFd,
) -> u8 {
    let result = match (src_remote, dst_remote) {
        (Some((user, host, rpath)), None) => {
            // download: remote -> local
            let conn = Conn { user, host, port: opts.port, identity: opts.identity.clone(), home: home.to_path_buf(), password_env };
            let local = resolve_local(cwd, &opts.dst);
            match connect_and_open(&conn).await {
                Ok(sftp) => download(&sftp, &rpath, &local, opts.recursive, fd1).await,
                Err(e) => Err(e),
            }
        }
        (None, Some((user, host, rpath))) => {
            // upload: local -> remote
            let conn = Conn { user, host, port: opts.port, identity: opts.identity.clone(), home: home.to_path_buf(), password_env };
            let local = resolve_local(cwd, &opts.src);
            match connect_and_open(&conn).await {
                Ok(sftp) => upload(&sftp, &local, &rpath, opts.recursive, fd1).await,
                Err(e) => Err(e),
            }
        }
        (Some(_), Some(_)) => Err("scp: remote-to-remote copy is not supported".to_string()),
        (None, None) => Err("scp: one of SRC/DST must be [user@]host:path".to_string()),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            write_fd(fd2, format!("scp: {e}\n").as_bytes());
            1
        }
    }
}

fn resolve_local(cwd: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        cwd.join(pb)
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

async fn download(
    sftp: &SftpSession,
    rpath: &str,
    local: &Path,
    recursive: bool,
    fd1: RawFd,
) -> Result<(), String> {
    let is_dir = sftp
        .metadata(rpath.to_string())
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false);
    if is_dir {
        if !recursive {
            return Err(format!("{rpath} is a directory (use -r)"));
        }
        return download_dir(sftp, rpath, local, fd1).await;
    }
    // File: if local is an existing directory, drop the basename into it.
    let dest = if local.is_dir() {
        local.join(basename(rpath))
    } else {
        local.to_path_buf()
    };
    let data = sftp
        .read(rpath.to_string())
        .await
        .map_err(|e| format!("read {rpath}: {e}"))?;
    std::fs::write(&dest, &data).map_err(|e| format!("write {}: {e}", dest.display()))?;
    write_fd(fd1, format!("{rpath} -> {} ({} bytes)\n", dest.display(), data.len()).as_bytes());
    Ok(())
}

fn download_dir<'a>(
    sftp: &'a SftpSession,
    rdir: &'a str,
    local: &'a Path,
    fd1: RawFd,
) -> BoxFuture<'a, Result<(), String>> {
    Box::pin(async move {
        std::fs::create_dir_all(local).map_err(|e| format!("mkdir {}: {e}", local.display()))?;
        let entries = sftp
            .read_dir(rdir.to_string())
            .await
            .map_err(|e| format!("readdir {rdir}: {e}"))?;
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child_r = format!("{}/{}", rdir.trim_end_matches('/'), name);
            let child_l = local.join(&name);
            if entry.file_type().is_dir() {
                download_dir(sftp, &child_r, &child_l, fd1).await?;
            } else {
                let data = sftp
                    .read(child_r.clone())
                    .await
                    .map_err(|e| format!("read {child_r}: {e}"))?;
                std::fs::write(&child_l, &data)
                    .map_err(|e| format!("write {}: {e}", child_l.display()))?;
                write_fd(fd1, format!("{child_r} -> {}\n", child_l.display()).as_bytes());
            }
        }
        Ok(())
    })
}

async fn upload(
    sftp: &SftpSession,
    local: &Path,
    rpath: &str,
    recursive: bool,
    fd1: RawFd,
) -> Result<(), String> {
    if local.is_dir() {
        if !recursive {
            return Err(format!("{} is a directory (use -r)", local.display()));
        }
        return upload_dir(sftp, local, rpath, fd1).await;
    }
    // If the remote path is an existing directory, place the basename inside it.
    let remote_is_dir = sftp
        .metadata(rpath.to_string())
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false);
    let dest = if remote_is_dir {
        format!(
            "{}/{}",
            rpath.trim_end_matches('/'),
            local.file_name().and_then(|s| s.to_str()).unwrap_or("file")
        )
    } else {
        rpath.to_string()
    };
    let data = std::fs::read(local).map_err(|e| format!("read {}: {e}", local.display()))?;
    write_remote_file(sftp, &dest, &data).await?;
    write_fd(fd1, format!("{} -> {dest} ({} bytes)\n", local.display(), data.len()).as_bytes());
    Ok(())
}

fn upload_dir<'a>(
    sftp: &'a SftpSession,
    local: &'a Path,
    rdir: &'a str,
    fd1: RawFd,
) -> BoxFuture<'a, Result<(), String>> {
    Box::pin(async move {
        let _ = sftp.create_dir(rdir.to_string()).await; // ignore "already exists"
        let entries = std::fs::read_dir(local).map_err(|e| format!("read {}: {e}", local.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_l = entry.path();
            let child_r = format!("{}/{}", rdir.trim_end_matches('/'), name);
            if child_l.is_dir() {
                upload_dir(sftp, &child_l, &child_r, fd1).await?;
            } else {
                let data = std::fs::read(&child_l).map_err(|e| format!("read {}: {e}", child_l.display()))?;
                write_remote_file(sftp, &child_r, &data).await?;
                write_fd(fd1, format!("{} -> {child_r}\n", child_l.display()).as_bytes());
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// sftp (interactive REPL)
// ---------------------------------------------------------------------------

fn exec_sftp(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();
        let getenv = env_reader(&context);

        // parse: sftp [-P port] [-i id] [user@]host
        let mut port = 22;
        let mut identity = None;
        let mut host_spec = None;
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-P" => {
                    i += 1;
                    match argv.get(i).and_then(|p| p.parse().ok()) {
                        Some(p) => port = p,
                        None => return fail(&context, "sftp: -P needs a port", 2),
                    }
                }
                "-i" => {
                    i += 1;
                    match argv.get(i) {
                        Some(p) => identity = Some(PathBuf::from(p)),
                        None => return fail(&context, "sftp: -i needs a path", 2),
                    }
                }
                a if host_spec.is_none() && !a.starts_with('-') => host_spec = Some(a.to_string()),
                a => return fail(&context, &format!("sftp: unsupported option {a}"), 2),
            }
            i += 1;
        }
        let Some(host_spec) = host_spec else {
            return fail(&context, "usage: sftp [-P port] [-i id] [user@]host", 2);
        };
        let default_user = getenv("USER").unwrap_or_else(|| "root".to_string());
        let (user, host) = match host_spec.split_once('@') {
            Some((u, h)) => (u.to_string(), h.to_string()),
            None => (default_user, host_spec),
        };
        let conn = Conn {
            user,
            host,
            port,
            identity,
            home: getenv("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
            password_env: getenv("SSH_PASSWORD"),
        };
        let cwd = context.shell.working_dir().to_path_buf();

        let fd0 = borrow_fd(&context, 0);
        let fd1 = borrow_fd(&context, 1);
        let fd2 = borrow_fd(&context, 2);
        let (Some(fd0), Some(fd1), Some(fd2)) = (fd0, fd1, fd2) else {
            return fail(&context, "sftp: no terminal", 1);
        };

        let code = tokio::task::spawn_blocking(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(_) => return 255u8,
            };
            rt.block_on(run_sftp_repl(
                conn,
                cwd,
                fd0.as_raw_fd(),
                fd1.as_raw_fd(),
                fd2.as_raw_fd(),
            ))
        })
        .await
        .unwrap_or(255);

        Ok(ExecutionResult::new(code))
    })
}

async fn run_sftp_repl(conn: Conn, cwd: PathBuf, fd0: RawFd, fd1: RawFd, fd2: RawFd) -> u8 {
    let sftp = match connect_and_open(&conn).await {
        Ok(s) => s,
        Err(e) => {
            write_fd(fd2, format!("sftp: {e}\n").as_bytes());
            return 255;
        }
    };
    let mut rdir = sftp.canonicalize(".".to_string()).await.unwrap_or_else(|_| "/".to_string());
    let mut ldir = cwd;

    let mut stdin = match AsyncFd::try_from(fd0) {
        Ok(s) => s,
        Err(_) => return 255,
    };
    // Restore blocking mode on exit (AsyncFd sets O_NONBLOCK on the shared fd).
    let saved = unsafe { libc::fcntl(fd0, libc::F_GETFL) };

    write_fd(fd1, b"Connected. Type 'help' for commands, 'quit' to exit.\n");
    loop {
        write_fd(fd1, b"sftp> ");
        let Some(line) = read_line(&mut stdin).await else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg1 = parts.next();
        let arg2 = parts.next();
        match cmd {
            "quit" | "exit" | "bye" => break,
            "help" | "?" => {
                write_fd(fd1, b"commands: ls [dir], cd dir, pwd, get remote [local], put local [remote],\n         lls [dir], lcd dir, lpwd, mkdir dir, rm file, rmdir dir, rename a b, quit\n");
            }
            "pwd" => write_fd(fd1, format!("remote: {rdir}\n").as_bytes()),
            "lpwd" => write_fd(fd1, format!("local: {}\n", ldir.display()).as_bytes()),
            "cd" => {
                let target = join_remote(&rdir, arg1.unwrap_or("."));
                match sftp.canonicalize(target).await {
                    Ok(p) => rdir = p,
                    Err(e) => write_fd(fd2, format!("cd: {e}\n").as_bytes()),
                }
            }
            "lcd" => {
                let t = resolve_local(&ldir, arg1.unwrap_or("."));
                if t.is_dir() {
                    ldir = t;
                } else {
                    write_fd(fd2, b"lcd: not a directory\n");
                }
            }
            "ls" => {
                let target = arg1.map_or_else(|| rdir.clone(), |a| join_remote(&rdir, a));
                match sftp.read_dir(target).await {
                    Ok(entries) => {
                        let mut out = String::new();
                        for e in entries {
                            let slash = if e.file_type().is_dir() { "/" } else { "" };
                            out.push_str(&format!("{}{}\n", e.file_name(), slash));
                        }
                        write_fd(fd1, out.as_bytes());
                    }
                    Err(e) => write_fd(fd2, format!("ls: {e}\n").as_bytes()),
                }
            }
            "lls" => {
                let t = arg1.map_or_else(|| ldir.clone(), |a| resolve_local(&ldir, a));
                if let Ok(rd) = std::fs::read_dir(&t) {
                    let mut out = String::new();
                    for e in rd.flatten() {
                        let slash = if e.path().is_dir() { "/" } else { "" };
                        out.push_str(&format!("{}{}\n", e.file_name().to_string_lossy(), slash));
                    }
                    write_fd(fd1, out.as_bytes());
                }
            }
            "get" => {
                let Some(remote) = arg1 else {
                    write_fd(fd2, b"usage: get remote [local]\n");
                    continue;
                };
                let rpath = join_remote(&rdir, remote);
                let local = resolve_local(&ldir, arg2.unwrap_or_else(|| basename(remote)));
                match sftp.read(rpath.clone()).await {
                    Ok(data) => {
                        if let Err(e) = std::fs::write(&local, &data) {
                            write_fd(fd2, format!("get: write {}: {e}\n", local.display()).as_bytes());
                        } else {
                            write_fd(fd1, format!("fetched {} ({} bytes)\n", local.display(), data.len()).as_bytes());
                        }
                    }
                    Err(e) => write_fd(fd2, format!("get: {e}\n").as_bytes()),
                }
            }
            "put" => {
                let Some(localn) = arg1 else {
                    write_fd(fd2, b"usage: put local [remote]\n");
                    continue;
                };
                let local = resolve_local(&ldir, localn);
                let rpath = join_remote(&rdir, arg2.unwrap_or_else(|| basename(localn)));
                match std::fs::read(&local) {
                    Ok(data) => match write_remote_file(&sftp, &rpath, &data).await {
                        Ok(()) => write_fd(fd1, format!("uploaded {rpath} ({} bytes)\n", data.len()).as_bytes()),
                        Err(e) => write_fd(fd2, format!("put: {e}\n").as_bytes()),
                    },
                    Err(e) => write_fd(fd2, format!("put: read {}: {e}\n", local.display()).as_bytes()),
                }
            }
            "mkdir" => {
                if let Some(d) = arg1 {
                    if let Err(e) = sftp.create_dir(join_remote(&rdir, d)).await {
                        write_fd(fd2, format!("mkdir: {e}\n").as_bytes());
                    }
                }
            }
            "rmdir" => {
                if let Some(d) = arg1 {
                    if let Err(e) = sftp.remove_dir(join_remote(&rdir, d)).await {
                        write_fd(fd2, format!("rmdir: {e}\n").as_bytes());
                    }
                }
            }
            "rm" => {
                if let Some(f) = arg1 {
                    if let Err(e) = sftp.remove_file(join_remote(&rdir, f)).await {
                        write_fd(fd2, format!("rm: {e}\n").as_bytes());
                    }
                }
            }
            "rename" => match (arg1, arg2) {
                (Some(a), Some(b)) => {
                    if let Err(e) = sftp.rename(join_remote(&rdir, a), join_remote(&rdir, b)).await {
                        write_fd(fd2, format!("rename: {e}\n").as_bytes());
                    }
                }
                _ => write_fd(fd2, b"usage: rename old new\n"),
            },
            other => write_fd(fd2, format!("unknown command: {other} (try 'help')\n").as_bytes()),
        }
    }
    if saved >= 0 {
        unsafe { libc::fcntl(fd0, libc::F_SETFL, saved) };
    }
    let _ = sftp.close().await;
    0
}

/// Joins a remote path against the current remote dir (absolute paths win).
fn join_remote(base: &str, p: &str) -> String {
    if p.starts_with('/') {
        p.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), p)
    }
}

async fn read_line(stdin: &mut AsyncFd) -> Option<String> {
    let mut line: Vec<u8> = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match stdin.read(&mut b).await {
            Ok(0) => {
                return (!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned());
            }
            Ok(_) => match b[0] {
                // Either CR or LF ends the line; a following empty line (from a
                // CR/LF pair) is harmlessly skipped by the REPL.
                b'\n' | b'\r' => return Some(String::from_utf8_lossy(&line).into_owned()),
                0x7f | 0x08 => {
                    line.pop();
                }
                0x03 | 0x04 => return None, // Ctrl-C / Ctrl-D
                c => line.push(c),
            },
            Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn borrow_fd(context: &ExecutionContext<'_, DefaultShellExtensions>, n: i32) -> Option<OwnedFd> {
    context.try_fd(n.into()).and_then(|f| {
        f.try_borrow_as_fd()
            .ok()
            .and_then(|bfd| bfd.try_clone_to_owned().ok())
    })
}

fn fail(
    context: &ExecutionContext<'_, DefaultShellExtensions>,
    msg: &str,
    code: u8,
) -> Result<ExecutionResult, brush_core::Error> {
    let mut err = context.stderr();
    let _ = std::io::Write::write_all(&mut err, msg.as_bytes());
    let _ = std::io::Write::write_all(&mut err, b"\n");
    Ok(ExecutionResult::new(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_spec_download() {
        let r = parse_remote("user@host:/etc/hosts", "def").unwrap();
        assert_eq!(r, ("user".into(), "host".into(), "/etc/hosts".into()));
    }

    #[test]
    fn remote_spec_default_user_and_relpath() {
        let r = parse_remote("host:file.txt", "me").unwrap();
        assert_eq!(r, ("me".into(), "host".into(), "file.txt".into()));
    }

    #[test]
    fn remote_spec_empty_path_is_dot() {
        let r = parse_remote("host:", "me").unwrap();
        assert_eq!(r.2, ".");
    }

    #[test]
    fn local_path_is_not_remote() {
        assert!(parse_remote("./local:weird", "me").is_none());
        assert!(parse_remote("/abs/path", "me").is_none());
        assert!(parse_remote("relative.txt", "me").is_none());
    }

    #[test]
    fn scp_parse_direction() {
        let o = parse_scp(&["scp".into(), "host:/f".into(), "local".into()]).unwrap();
        assert!(parse_remote(&o.src, "u").is_some());
        assert!(parse_remote(&o.dst, "u").is_none());
    }

    #[test]
    fn scp_recursive_and_port() {
        let o = parse_scp(&[
            "scp".into(),
            "-r".into(),
            "-P".into(),
            "2222".into(),
            "d".into(),
            "host:/d".into(),
        ])
        .unwrap();
        assert!(o.recursive);
        assert_eq!(o.port, 2222);
    }

    #[test]
    fn join_remote_paths() {
        assert_eq!(join_remote("/home/me", "f"), "/home/me/f");
        assert_eq!(join_remote("/home/me", "/etc/x"), "/etc/x");
        assert_eq!(join_remote("/a/", "b"), "/a/b");
    }
}

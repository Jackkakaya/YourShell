//! Interactive `ssh` client, backed by the pure-Rust russh library (Apache-2.0,
//! `ring` crypto backend so it cross-compiles to iOS like ureq/rustls).
//!
//! Unlike the uutils/awk/python adapters this does NOT touch process-global
//! stdio or the `process_state_lock`: russh reads and writes the session's fds
//! directly via async, so an ssh session is entirely self-contained with no
//! global state to corrupt. The flow:
//!
//!   1. parse `[user@]host [-p port] [-i identity] [command...]`
//!   2. TCP connect + SSH handshake (russh)
//!   3. authenticate: each unencrypted key in ~/.ssh, then $SSH_PASSWORD, then
//!      an interactive no-echo password prompt (read raw off fd 0)
//!   4. interactive: emit the alternate-screen enter sequence (which flips the
//!      Swift side into raw key passthrough), request a PTY + shell, then a
//!      `tokio::select!` loop bridges fd 0 -> channel and channel -> fd 1
//!      until the remote shell exits; `ssh host <cmd>` skips the PTY and streams
//!   5. leave the alternate screen and restore the fds' blocking mode
//!
//! Because the shell's own tokio runtime may not have the IO driver enabled,
//! the whole session runs on a dedicated `enable_all` current-thread runtime
//! inside `spawn_blocking` (the owned fds are `Send`).

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;
use russh::client::{self, Handler};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_fd::AsyncFd;

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_ssh,
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
    Ok(format!("{name}: ssh client (russh, in-process)"))
}

/// Parsed command line.
struct Opts {
    user: String,
    host: String,
    port: u16,
    identity: Option<PathBuf>,
    command: Option<String>,
    home: PathBuf,
    term: String,
    cols: u32,
    rows: u32,
    password_env: Option<String>,
}

fn exec_ssh(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();

        let getenv = |k: &str| -> Option<String> {
            context
                .shell
                .env()
                .get_str(k, context.shell)
                .map(|c| c.into_owned())
        };

        let opts = match parse_opts(&argv, &getenv) {
            Ok(o) => o,
            Err(msg) => {
                let mut err = context.stderr();
                let _ = std::io::Write::write_all(&mut err, msg.as_bytes());
                let _ = std::io::Write::write_all(&mut err, b"\n");
                return Ok(ExecutionResult::new(2));
            }
        };

        let fds: [Option<OwnedFd>; 3] = [0, 1, 2].map(|n| {
            context.try_fd(n.into()).and_then(|f| {
                f.try_borrow_as_fd()
                    .ok()
                    .and_then(|bfd| bfd.try_clone_to_owned().ok())
            })
        });
        let [fd0, fd1, fd2] = fds;
        let (Some(fd0), Some(fd1), Some(fd2)) = (fd0, fd1, fd2) else {
            let mut err = context.stderr();
            let _ = std::io::Write::write_all(&mut err, b"ssh: no controlling terminal\n");
            return Ok(ExecutionResult::new(1));
        };

        let code = tokio::task::spawn_blocking(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return 255,
            };
            rt.block_on(run_session(opts, fd0, fd1, fd2))
        })
        .await
        .unwrap_or(255);

        Ok(ExecutionResult::new(code))
    })
}

fn parse_opts(argv: &[String], getenv: &dyn Fn(&str) -> Option<String>) -> Result<Opts, String> {
    let mut host_spec: Option<String> = None;
    let mut port: u16 = 22;
    let mut identity: Option<PathBuf> = None;
    let mut command_parts: Vec<String> = Vec::new();
    let mut i = 1; // argv[0] is "ssh"
    while i < argv.len() {
        let a = &argv[i];
        match a.as_str() {
            "-p" => {
                i += 1;
                port = argv
                    .get(i)
                    .and_then(|p| p.parse().ok())
                    .ok_or("ssh: -p needs a port number")?;
            }
            "-i" => {
                i += 1;
                identity = Some(PathBuf::from(
                    argv.get(i).ok_or("ssh: -i needs a key path")?,
                ));
            }
            _ if host_spec.is_none() && !a.starts_with('-') => host_spec = Some(a.clone()),
            _ if host_spec.is_some() => command_parts.push(a.clone()),
            _ => return Err(format!("ssh: unsupported option {a}")),
        }
        i += 1;
    }
    let host_spec = host_spec.ok_or("usage: ssh [-p port] [-i identity] [user@]host [command]")?;
    let (user, host) = match host_spec.split_once('@') {
        Some((u, h)) => (u.to_string(), h.to_string()),
        None => (
            getenv("USER").unwrap_or_else(|| "root".to_string()),
            host_spec,
        ),
    };
    let home = getenv("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let cols = getenv("COLUMNS").and_then(|v| v.parse().ok()).unwrap_or(80);
    let rows = getenv("LINES").and_then(|v| v.parse().ok()).unwrap_or(24);
    Ok(Opts {
        user,
        host,
        port,
        identity,
        command: (!command_parts.is_empty()).then(|| command_parts.join(" ")),
        home,
        term: getenv("TERM").unwrap_or_else(|| "xterm-256color".to_string()),
        cols,
        rows,
        password_env: getenv("SSH_PASSWORD"),
    })
}

/// Accepts any host key (TOFU/insecure). TODO: verify against ~/.ssh/known_hosts
/// once we surface the fingerprint prompt through the raw channel.
struct ClientHandler;

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

async fn run_session(opts: Opts, fd0: OwnedFd, fd1: OwnedFd, fd2: OwnedFd) -> u8 {
    let raw0 = fd0.as_raw_fd();
    let raw1 = fd1.as_raw_fd();
    let raw2 = fd2.as_raw_fd();

    macro_rules! eprint_fd {
        ($($arg:tt)*) => {{
            let s = format!($($arg)*);
            unsafe { libc::write(raw2, s.as_ptr().cast(), s.len()); }
        }};
    }

    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(3600)),
        ..Default::default()
    });

    let mut session = match client::connect(config, (opts.host.as_str(), opts.port), ClientHandler)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            eprint_fd!("ssh: connect {}:{} failed: {e}\n", opts.host, opts.port);
            return 255;
        }
    };

    // --- Authentication ---------------------------------------------------
    let mut authed = false;

    // 1. Public keys from -i or the usual ~/.ssh names (unencrypted only).
    let mut key_paths: Vec<PathBuf> = Vec::new();
    if let Some(id) = &opts.identity {
        key_paths.push(id.clone());
    } else {
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            key_paths.push(opts.home.join(".ssh").join(name));
        }
    }
    for path in key_paths {
        if !path.exists() {
            continue;
        }
        let key = match load_secret_key(&path, None) {
            Ok(k) => k,
            Err(_) => continue, // encrypted or unreadable — skip
        };
        let hash = session.best_supported_rsa_hash().await.ok().flatten().flatten();
        if let Ok(res) = session
            .authenticate_publickey(&opts.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
        {
            if res.success() {
                authed = true;
                break;
            }
        }
    }

    // 2. $SSH_PASSWORD, then an interactive no-echo prompt (raw off fd 0).
    if !authed {
        // Enter the alternate screen now so the password prompt reads raw
        // keystrokes without local echo.
        let _ = write_all_fd(raw1, b"\x1b[?1049h");
        let restore = FdFlagsGuard::capture(&[raw0, raw1]);

        for attempt in 0..3 {
            let password = if attempt == 0 && opts.password_env.is_some() {
                opts.password_env.clone().unwrap()
            } else {
                let prompt = format!("\r\n{}@{}'s password: ", opts.user, opts.host);
                let _ = write_all_fd(raw1, prompt.as_bytes());
                match read_password_raw(raw0).await {
                    Some(p) => p,
                    None => break,
                }
            };
            match session.authenticate_password(&opts.user, &password).await {
                Ok(res) if res.success() => {
                    authed = true;
                    break;
                }
                Ok(_) => {
                    let _ = write_all_fd(raw1, b"\r\nPermission denied, please try again.");
                }
                Err(e) => {
                    eprint_fd!("\r\nssh: auth error: {e}\n");
                    break;
                }
            }
        }
        drop(restore);
        if !authed {
            let _ = write_all_fd(raw1, b"\x1b[?1049l");
        }
    }

    if !authed {
        eprint_fd!("ssh: authentication failed for {}@{}\n", opts.user, opts.host);
        return 255;
    }

    // --- Channel + PTY/shell or remote command ----------------------------
    let mut channel = match session.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            eprint_fd!("ssh: open channel failed: {e}\n");
            return 255;
        }
    };

    let interactive = opts.command.is_none();
    if interactive {
        if let Err(e) = channel
            .request_pty(false, &opts.term, opts.cols, opts.rows, 0, 0, &[])
            .await
        {
            eprint_fd!("ssh: request_pty failed: {e}\n");
            return 255;
        }
        // Ensure the alternate screen / raw passthrough is active (it already is
        // if we went through the password prompt above).
        let _ = write_all_fd(raw1, b"\x1b[?1049h");
        if let Err(e) = channel.request_shell(true).await {
            eprint_fd!("ssh: request_shell failed: {e}\n");
            return 255;
        }
    } else if let Err(e) = channel.exec(true, opts.command.as_deref().unwrap()).await {
        eprint_fd!("ssh: exec failed: {e}\n");
        return 255;
    }

    let restore = FdFlagsGuard::capture(&[raw0, raw1]);
    let code = pump(&mut channel, raw0, raw1, raw2, interactive).await;
    drop(restore);
    if interactive {
        let _ = write_all_fd(raw1, b"\x1b[?1049l");
    }
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    code
}

/// The bidirectional data loop: fd 0 -> channel, channel -> fd 1/2.
async fn pump(
    channel: &mut russh::Channel<client::Msg>,
    raw0: RawFd,
    raw1: RawFd,
    raw2: RawFd,
    forward_stdin: bool,
) -> u8 {
    let mut stdin = AsyncFd::try_from(raw0).ok();
    let mut stdout = match AsyncFd::try_from(raw1) {
        Ok(s) => s,
        Err(_) => return 255,
    };
    let mut stderr = AsyncFd::try_from(raw2).ok();
    let mut buf = vec![0u8; 8192];
    let mut stdin_closed = !forward_stdin;
    let mut code: u8 = 0;

    loop {
        tokio::select! {
            r = async { stdin.as_mut().unwrap().read(&mut buf).await }, if !stdin_closed => {
                match r {
                    Ok(0) => { stdin_closed = true; let _ = channel.eof().await; }
                    Ok(n) => { if channel.data(&buf[..n]).await.is_err() { break; } }
                    Err(_) => { stdin_closed = true; let _ = channel.eof().await; }
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { ref data }) => {
                        if stdout.write_all(data).await.is_err() { break; }
                        let _ = stdout.flush().await;
                    }
                    Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                        if let Some(e) = stderr.as_mut() {
                            let _ = e.write_all(data).await;
                            let _ = e.flush().await;
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        code = (exit_status & 0xff) as u8;
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        }
    }
    code
}

/// Reads a password from a raw fd character-by-character (no echo), until CR/LF.
async fn read_password_raw(raw0: RawFd) -> Option<String> {
    let mut stdin = AsyncFd::try_from(raw0).ok()?;
    let mut pw = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match stdin.read(&mut b).await {
            Ok(0) => return None,
            Ok(_) => match b[0] {
                b'\r' | b'\n' => return Some(String::from_utf8_lossy(&pw).into_owned()),
                0x7f | 0x08 => {
                    pw.pop();
                }
                0x03 => return None, // Ctrl-C
                c => pw.push(c),
            },
            Err(_) => return None,
        }
    }
}

fn write_all_fd(fd: RawFd, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n <= 0 {
            return Err(std::io::Error::last_os_error());
        }
        data = &data[n as usize..];
    }
    Ok(())
}

/// Saves the O_NONBLOCK/status flags of some fds and restores them on drop, so
/// the shell's line reader isn't left with a non-blocking stdin after ssh (the
/// tokio AsyncFd wrappers set O_NONBLOCK on the shared descriptions).
struct FdFlagsGuard {
    saved: Vec<(RawFd, i32)>,
}

impl FdFlagsGuard {
    fn capture(fds: &[RawFd]) -> Self {
        let saved = fds
            .iter()
            .filter_map(|&fd| {
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
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, flags);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn parse(args: &[&str], env: &dyn Fn(&str) -> Option<String>) -> Result<Opts, String> {
        let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        parse_opts(&argv, env)
    }

    #[test]
    fn user_at_host() {
        let env = env_of(&[("HOME", "/home/me")]);
        let o = parse(&["ssh", "alice@example.com"], &env).unwrap();
        assert_eq!(o.user, "alice");
        assert_eq!(o.host, "example.com");
        assert_eq!(o.port, 22);
        assert!(o.command.is_none());
        assert!(o.identity.is_none());
    }

    #[test]
    fn bare_host_uses_env_user() {
        let env = env_of(&[("USER", "bob"), ("HOME", "/h")]);
        let o = parse(&["ssh", "host1"], &env).unwrap();
        assert_eq!(o.user, "bob");
        assert_eq!(o.host, "host1");
    }

    #[test]
    fn port_and_identity_and_command() {
        let env = env_of(&[("HOME", "/h")]);
        let o = parse(
            &["ssh", "-p", "2222", "-i", "/h/.ssh/k", "u@h", "uname", "-a"],
            &env,
        )
        .unwrap();
        assert_eq!(o.port, 2222);
        assert_eq!(o.identity.as_deref(), Some(std::path::Path::new("/h/.ssh/k")));
        assert_eq!(o.command.as_deref(), Some("uname -a"));
    }

    #[test]
    fn pty_size_from_env() {
        let env = env_of(&[("HOME", "/h"), ("COLUMNS", "120"), ("LINES", "40")]);
        let o = parse(&["ssh", "h"], &env).unwrap();
        assert_eq!((o.cols, o.rows), (120, 40));
    }

    #[test]
    fn missing_host_is_error() {
        let env = env_of(&[("HOME", "/h")]);
        assert!(parse(&["ssh"], &env).is_err());
        assert!(parse(&["ssh", "-p", "22"], &env).is_err());
    }

    #[test]
    fn bad_port_is_error() {
        let env = env_of(&[("HOME", "/h")]);
        assert!(parse(&["ssh", "-p", "notaport", "h"], &env).is_err());
    }
}

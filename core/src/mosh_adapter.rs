//! `mosh` — a from-scratch pure-Rust mosh client (there is no reusable Rust
//! mosh library). Client-only: it never listens; it either bootstraps a remote
//! mosh-server over ssh, or (for testing / MOSH_KEY use) connects directly to
//! an already-running server's UDP port.
//!
//! This file currently holds the bootstrap layer (Phase 1): argument parsing
//! and the `MOSH CONNECT <port> <key>` parser. The UDP transport, AES-OCB
//! datagram crypto, SSP state-sync, protobuf messages and terminal model are
//! added in later phases.

use std::path::PathBuf;

pub(crate) mod wire;

/// How the client obtains the server endpoint + session key.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// `mosh [user@]host` — ssh in, run `mosh-server new`, parse its output.
    Bootstrap { user: String, host: String, ssh_port: u16 },
    /// `mosh --port <p> <host>` with `MOSH_KEY` in the env — connect directly
    /// to a running server (this is how the real `mosh-client` is invoked, and
    /// how we test against a locally started mosh-server).
    Direct { host: String, udp_port: u16, key: String },
}

#[derive(Debug, PartialEq, Eq)]
struct Opts {
    mode: Mode,
    identity: Option<PathBuf>,
    home: PathBuf,
    term: String,
    cols: u16,
    rows: u16,
}

/// Parsed `MOSH CONNECT <port> <key>` line printed by `mosh-server new`.
#[derive(Debug, PartialEq, Eq)]
struct MoshConnect {
    port: u16,
    /// Base64 (no padding) of the 16-byte AES-128 session key, as printed.
    key: String,
}

/// Scans mosh-server's stdout for the `MOSH CONNECT <port> <key>` line.
fn parse_mosh_connect(output: &str) -> Option<MoshConnect> {
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("MOSH CONNECT ") else {
            continue;
        };
        let mut it = rest.split_whitespace();
        let port: u16 = it.next()?.parse().ok()?;
        let key = it.next()?.to_string();
        // The key is base64 of a 16-byte key: 22 chars unpadded.
        if key.len() == 22 {
            return Some(MoshConnect { port, key });
        }
    }
    None
}

/// The command mosh runs on the remote to spawn a server. `-s` binds to the
/// SSH connection's address; `-c 256` allows 256-color; `-l` forwards a locale.
fn mosh_server_command(locale: &str) -> String {
    format!("mosh-server new -s -c 256 -l LANG={locale}")
}

fn parse_opts(
    argv: &[String],
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<Opts, String> {
    let mut ssh_port = 22;
    let mut udp_port: Option<u16> = None;
    let mut identity = None;
    let mut host_spec = None;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "-p" | "--ssh-port" => {
                i += 1;
                ssh_port = argv
                    .get(i)
                    .and_then(|p| p.parse().ok())
                    .ok_or("mosh: -p needs a port")?;
            }
            "--port" | "-P" => {
                i += 1;
                udp_port = Some(
                    argv.get(i)
                        .and_then(|p| p.parse().ok())
                        .ok_or("mosh: --port needs a UDP port")?,
                );
            }
            "-i" => {
                i += 1;
                identity = Some(PathBuf::from(argv.get(i).ok_or("mosh: -i needs a path")?));
            }
            a if host_spec.is_none() && !a.starts_with('-') => host_spec = Some(a.to_string()),
            a => return Err(format!("mosh: unsupported option {a}")),
        }
        i += 1;
    }
    let host_spec = host_spec.ok_or("usage: mosh [-p ssh_port] [-i id] [user@]host")?;
    let (user, host) = match host_spec.split_once('@') {
        Some((u, h)) => (u.to_string(), h.to_string()),
        None => (
            getenv("USER").unwrap_or_else(|| "root".to_string()),
            host_spec,
        ),
    };

    // Direct mode when a UDP port + MOSH_KEY are supplied (mosh-client style).
    let mode = match (udp_port, getenv("MOSH_KEY")) {
        (Some(udp_port), Some(key)) => Mode::Direct { host, udp_port, key },
        (Some(_), None) => return Err("mosh: --port given but MOSH_KEY not set".to_string()),
        (None, _) => Mode::Bootstrap { user, host, ssh_port },
    };

    Ok(Opts {
        mode,
        identity,
        home: getenv("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
        term: getenv("TERM").unwrap_or_else(|| "xterm-256color".to_string()),
        cols: getenv("COLUMNS").and_then(|v| v.parse().ok()).unwrap_or(80),
        rows: getenv("LINES").and_then(|v| v.parse().ok()).unwrap_or(24),
    })
}

// ===========================================================================
// Interactive client (Phases 3–5): SSP receive loop + terminal + user input.
// ===========================================================================

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::time::{Duration, SystemTime};

use base64::Engine;
use brush_core::builtins::Registration;
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tokio_fd::AsyncFd;

use wire::{build_client_datagram, decode_host_message, Crypto, Instruction, PROTOCOL_VERSION};

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_mosh,
        content_func: |name, _, _| Ok(format!("{name}: mosh client (pure Rust, in-process)")),
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

fn exec_mosh(
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
            Err(m) => return fail(&context, &m, 2),
        };

        let fd0 = borrow_fd(&context, 0);
        let fd1 = borrow_fd(&context, 1);
        let fd2 = borrow_fd(&context, 2);
        let (Some(fd0), Some(fd1), Some(fd2)) = (fd0, fd1, fd2) else {
            return fail(&context, "mosh: no terminal", 1);
        };

        let code = tokio::task::spawn_blocking(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(_) => return 255u8,
            };
            rt.block_on(run(opts, fd0, fd1, fd2))
        })
        .await
        .unwrap_or(255);
        Ok(ExecutionResult::new(code))
    })
}

async fn run(opts: Opts, fd0: OwnedFd, fd1: OwnedFd, fd2: OwnedFd) -> u8 {
    let raw2 = fd2.as_raw_fd();
    macro_rules! eprint_fd {
        ($($a:tt)*) => {{ let s = format!($($a)*); unsafe { libc::write(raw2, s.as_ptr().cast(), s.len()); } }};
    }

    // Resolve endpoint + key.
    let (host, udp_port, key_b64) = match &opts.mode {
        Mode::Direct { host, udp_port, key } => (host.clone(), *udp_port, key.clone()),
        Mode::Bootstrap { user, host, ssh_port } => {
            match bootstrap_over_ssh(user, host, *ssh_port, &opts).await {
                Ok((port, key)) => (host.clone(), port, key),
                Err(e) => {
                    eprint_fd!("mosh: {e}\n");
                    return 255;
                }
            }
        }
    };

    let key_bytes = match base64::engine::general_purpose::STANDARD_NO_PAD.decode(key_b64.trim()) {
        Ok(k) if k.len() == 16 => k,
        _ => {
            eprint_fd!("mosh: invalid MOSH key\n");
            return 255;
        }
    };
    let mut key = [0u8; 16];
    key.copy_from_slice(&key_bytes);
    let crypto = Crypto::new(&key);

    let sock = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            eprint_fd!("mosh: bind: {e}\n");
            return 255;
        }
    };
    if let Err(e) = sock.connect(format!("{host}:{udp_port}")).await {
        eprint_fd!("mosh: connect {host}:{udp_port}: {e}\n");
        return 255;
    }

    let raw0 = fd0.as_raw_fd();
    let raw1 = fd1.as_raw_fd();
    // Enter the alternate screen (flips Swift into raw key passthrough).
    let _ = write_fd(raw1, b"\x1b[?1049h");
    let restore = FdFlagsGuard::capture(&[raw0, raw1]);

    let code = client_loop(
        &crypto, sock, &host, udp_port, raw0, raw1, opts.cols, opts.rows,
    )
    .await;

    drop(restore);
    // Reset any global private modes the remote left on, then leave alt screen
    // (see ssh_adapter::TERM_CLEANUP).
    let _ = write_fd(raw1, crate::ssh_adapter::TERM_CLEANUP);
    let _ = write_fd(raw1, b"\x1b[?1049l");
    code
}

/// Client transport state. The client→server direction synchronizes a
/// UserStream (an ordered list of `UserEvent`s); each state number maps to how
/// many events existed at that point, so a diff to `old_num` is just the events
/// after that state's count.
struct ClientState {
    seq: u64,     // outgoing datagram sequence
    frag_id: u64, // outgoing fragment id
    events: Vec<wire::UserEvent>,
    input_num: u64,   // our latest user-state number
    assumed_ack: u64, // highest input_num the server has acked
    // (state number, event count at that state); starts at (0, 0).
    states: Vec<(u64, usize)>,
    last_recv_num: u64, // highest server state we've applied
    last_recv_ts: u16,  // last server timestamp (for timestamp_reply)
}

impl ClientState {
    fn new() -> Self {
        Self {
            seq: 0,
            frag_id: 0,
            events: Vec::new(),
            input_num: 0,
            assumed_ack: 0,
            states: vec![(0, 0)],
            last_recv_num: 0,
            last_recv_ts: 0,
        }
    }

    fn event_count_for(&self, num: u64) -> usize {
        self.states
            .iter()
            .rev()
            .find(|(n, _)| *n <= num)
            .map_or(0, |(_, c)| *c)
    }

    /// Records a new event as a fresh state.
    fn push_event(&mut self, ev: wire::UserEvent) {
        self.events.push(ev);
        self.input_num += 1;
        self.states.push((self.input_num, self.events.len()));
    }
}

fn now_ms() -> u16 {
    (SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        % 65536) as u16
}

/// Sends the current client state as one datagram (diff = the UserStream events
/// after the server's assumed state).
async fn send_state(crypto: &Crypto, sock: &UdpSocket, st: &mut ClientState) {
    let old_count = st.event_count_for(st.assumed_ack);
    let new_events = &st.events[old_count..];
    let diff = if new_events.is_empty() {
        Vec::new()
    } else {
        wire::encode_user_message(new_events)
    };
    let inst = Instruction {
        protocol_version: PROTOCOL_VERSION,
        old_num: st.assumed_ack,
        new_num: st.input_num,
        ack_num: st.last_recv_num,
        throwaway_num: 0,
        diff,
        chaff: Vec::new(),
    };
    st.seq += 1;
    st.frag_id += 1;
    let dg = build_client_datagram(crypto, st.seq, now_ms(), st.last_recv_ts, st.frag_id, &inst);
    let _ = sock.send(&dg).await;
}

/// Rebinds a fresh UDP socket to the same server — used for roaming when the
/// local network changes (new interface/address). The mosh-server learns the
/// new client address from the next authenticated datagram, so the encrypted
/// session (key + sequence numbers) simply continues over the new socket.
async fn rebind(host: &str, port: u16) -> std::io::Result<UdpSocket> {
    let s = UdpSocket::bind("0.0.0.0:0").await?;
    s.connect((host, port)).await?;
    Ok(s)
}

/// True for errors that mean the local network path changed/went away (as
/// opposed to the peer refusing) — the cue to roam onto a new socket.
fn is_network_change(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(
            libc::ENETUNREACH
                | libc::EHOSTUNREACH
                | libc::ENETDOWN
                | libc::ENETRESET
                | libc::EADDRNOTAVAIL
        )
    )
}

async fn client_loop(
    crypto: &Crypto,
    mut sock: UdpSocket,
    host: &str,
    udp_port: u16,
    raw0: RawFd,
    raw1: RawFd,
    cols: u16,
    rows: u16,
) -> u8 {
    let mut stdin = match AsyncFd::try_from(raw0) {
        Ok(s) => s,
        Err(_) => return 255,
    };
    let mut st = ClientState::new();
    let mut assembler = wire::FragmentAssembler::new();

    // Announce ourselves, then send the initial window size as the first event.
    send_state(crypto, &sock, &mut st).await;
    st.push_event(wire::UserEvent::Resize(cols as i32, rows as i32));
    send_state(crypto, &sock, &mut st).await;

    // Debug: bound the session length so headless tests can flush the transcript.
    let max_ticks = std::env::var("MOSH_MAX_SECS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .map(|s| s * 10);
    // Debug: force a roam at this tick to exercise the roaming path.
    let roam_at_tick = std::env::var("MOSH_ROAM_AFTER")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .map(|s| s * 10);

    let mut kbuf = [0u8; 4096];
    let mut dbuf = [0u8; 2048];
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut total_ticks = 0u32;
    let mut silent_ticks = 0u32;
    let mut recv_errors = 0u32; // consecutive ECONNREFUSED (server gone)
    let mut connected = false; // received at least one server datagram
    let mut escape_armed = false; // Ctrl-^ then '.' quits

    loop {
        tokio::select! {
            r = stdin.read(&mut kbuf) => {
                match r {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // mosh escape: Ctrl-^ (0x1e) then '.' to quit.
                        for &b in &kbuf[..n] {
                            if escape_armed {
                                escape_armed = false;
                                if b == b'.' { return 0; }
                            } else if b == 0x1e {
                                escape_armed = true;
                                continue;
                            }
                        }
                        st.push_event(wire::UserEvent::Keys(kbuf[..n].to_vec()));
                        send_state(crypto, &sock, &mut st).await;
                    }
                }
            }
            r = sock.recv(&mut dbuf) => {
                match r {
                    Ok(n) => {
                        recv_errors = 0;
                        // Decrypt, reassemble fragments (large screen updates span
                        // multiple datagrams), then decode the Instruction.
                        if let Some(opened) = crypto.open(&dbuf[..n]) {
                            connected = true;
                            silent_ticks = 0;
                            st.last_recv_ts = opened.timestamp;
                            if let Some((id, num, is_final, contents)) =
                                wire::parse_fragment(&opened.payload)
                            {
                                if let Some(compressed) = assembler.add(id, num, is_final, contents) {
                                    if let Some(inst) = wire::zlib_decompress(&compressed)
                                        .and_then(|raw| Instruction::decode(&raw))
                                    {
                                        if inst.ack_num > st.assumed_ack {
                                            st.assumed_ack = inst.ack_num;
                                        }
                                        if inst.new_num > st.last_recv_num {
                                            let bytes = decode_host_message(&inst.diff);
                                            if !bytes.is_empty() {
                                                let _ = write_fd(raw1, &bytes);
                                            }
                                            st.last_recv_num = inst.new_num;
                                            // Ack the freshly applied state.
                                            send_state(crypto, &sock, &mut st).await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if is_network_change(&e) {
                            // The local network path changed (Wi-Fi↔cellular,
                            // new address). Roam onto a fresh socket and keep the
                            // same encrypted session; the server learns our new
                            // address from the next authenticated datagram.
                            if let Ok(ns) = rebind(host, udp_port).await {
                                sock = ns;
                                send_state(crypto, &sock, &mut st).await;
                            }
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        } else {
                            // ECONNREFUSED (ICMP port-unreachable) the moment
                            // mosh-server exits — the remote shell quit. Don't
                            // reset the silence timer; after a short burst, end
                            // the session cleanly.
                            recv_errors += 1;
                            if connected && recv_errors > 5 {
                                return 0;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
            _ = tick.tick() => {
                total_ticks += 1;
                silent_ticks += 1;
                // Retransmit unacked input every ~200ms.
                if st.input_num > st.assumed_ack && silent_ticks % 2 == 0 {
                    send_state(crypto, &sock, &mut st).await;
                }
                // Heartbeat/ack every ~2s of quiet.
                if silent_ticks % 20 == 0 {
                    send_state(crypto, &sock, &mut st).await;
                }
                // Once connected, ~5s of server silence means the remote shell
                // exited and mosh-server terminated (both sides otherwise
                // heartbeat every ~3s) — exit cleanly.
                if connected && silent_ticks > 50 {
                    return 0;
                }
                // Never connected after ~15s: UDP is likely blocked.
                if !connected && total_ticks > 150 {
                    return 255;
                }
                if let Some(max) = max_ticks {
                    if total_ticks >= max {
                        return 0;
                    }
                }
                // Debug: force a roam (rebind onto a new local port) at a fixed
                // time to exercise the roaming path in headless tests.
                if let Some(at) = roam_at_tick {
                    if total_ticks == at {
                        if let Ok(ns) = rebind(host, udp_port).await {
                            sock = ns;
                            send_state(crypto, &sock, &mut st).await;
                        }
                    }
                }
            }
        }
    }
    0
}

/// Bootstrap: ssh to the host, run `mosh-server new`, parse MOSH CONNECT.
async fn bootstrap_over_ssh(
    user: &str,
    host: &str,
    ssh_port: u16,
    opts: &Opts,
) -> Result<(u16, String), String> {
    use crate::ssh_adapter::{connect_session, key_and_env_auth, HostKeyOutcome};
    use tokio::io::AsyncReadExt as _;

    let (res, hostkey) = connect_session(host, ssh_port, &opts.home).await;
    let mut session = res.map_err(|e| {
        if hostkey == HostKeyOutcome::Changed {
            format!("host key changed for {host} — refusing (see ssh)")
        } else {
            format!("ssh connect {host}:{ssh_port}: {e}")
        }
    })?;
    let password_env = std::env::var("SSH_PASSWORD").ok();
    if !key_and_env_auth(
        &mut session,
        user,
        &opts.home,
        opts.identity.as_deref(),
        password_env.as_deref(),
    )
    .await
    {
        return Err(format!("ssh auth failed for {user}@{host}"));
    }
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("open channel: {e}"))?;
    let cmd = mosh_server_command("en_US.UTF-8");
    channel
        .exec(true, cmd.as_str())
        .await
        .map_err(|e| format!("exec mosh-server: {e}"))?;

    // Collect stdout until we see the MOSH CONNECT line.
    let mut stream = channel.into_stream();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(c) = parse_mosh_connect(&String::from_utf8_lossy(&buf)) {
                    return Ok((c.port, c.key));
                }
            }
            Ok(Err(_)) => break,
        }
    }
    parse_mosh_connect(&String::from_utf8_lossy(&buf))
        .map(|c| (c.port, c.key))
        .ok_or_else(|| "mosh-server did not print MOSH CONNECT (is mosh installed on the remote?)".to_string())
}

fn write_fd(fd: RawFd, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n <= 0 {
            return Err(std::io::Error::last_os_error());
        }
        data = &data[n as usize..];
    }
    Ok(())
}

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

/// Restores O_NONBLOCK flags on drop (AsyncFd sets them on the shared fds).
struct FdFlagsGuard {
    saved: Vec<(RawFd, i32)>,
}
impl FdFlagsGuard {
    fn capture(fds: &[RawFd]) -> Self {
        let saved = fds
            .iter()
            .filter_map(|&fd| {
                let f = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                (f >= 0).then_some((fd, f))
            })
            .collect();
        Self { saved }
    }
}
impl Drop for FdFlagsGuard {
    fn drop(&mut self) {
        for &(fd, f) in &self.saved {
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    fn parse(args: &[&str], env: &dyn Fn(&str) -> Option<String>) -> Result<Opts, String> {
        let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        parse_opts(&argv, env)
    }

    #[test]
    fn connect_line_parsed() {
        let out = "Warning: SSH_CONNECTION not found\nMOSH CONNECT 60001 jrFRIoAdtzhusT8CUraUng\n\nmosh-server (mosh 1.4.0)";
        let c = parse_mosh_connect(out).unwrap();
        assert_eq!(c.port, 60001);
        assert_eq!(c.key, "jrFRIoAdtzhusT8CUraUng");
        assert_eq!(c.key.len(), 22);
    }

    #[test]
    fn connect_line_absent() {
        assert!(parse_mosh_connect("no connect line here").is_none());
        // Wrong key length is rejected.
        assert!(parse_mosh_connect("MOSH CONNECT 60001 shortkey").is_none());
    }

    #[test]
    fn server_command_shape() {
        assert_eq!(
            mosh_server_command("en_US.UTF-8"),
            "mosh-server new -s -c 256 -l LANG=en_US.UTF-8"
        );
    }

    #[test]
    fn bootstrap_mode_default() {
        let env = env_of(&[("HOME", "/h"), ("USER", "me")]);
        let o = parse(&["mosh", "example.com"], &env).unwrap();
        assert_eq!(
            o.mode,
            Mode::Bootstrap { user: "me".into(), host: "example.com".into(), ssh_port: 22 }
        );
    }

    #[test]
    fn bootstrap_user_and_ssh_port() {
        let env = env_of(&[("HOME", "/h")]);
        let o = parse(&["mosh", "-p", "2222", "alice@h"], &env).unwrap();
        assert_eq!(
            o.mode,
            Mode::Bootstrap { user: "alice".into(), host: "h".into(), ssh_port: 2222 }
        );
    }

    #[test]
    fn direct_mode_with_key() {
        let env = env_of(&[("HOME", "/h"), ("MOSH_KEY", "jrFRIoAdtzhusT8CUraUng")]);
        let o = parse(&["mosh", "--port", "60001", "127.0.0.1"], &env).unwrap();
        assert_eq!(
            o.mode,
            Mode::Direct {
                host: "127.0.0.1".into(),
                udp_port: 60001,
                key: "jrFRIoAdtzhusT8CUraUng".into()
            }
        );
    }

    #[test]
    fn direct_mode_requires_key() {
        let env = env_of(&[("HOME", "/h")]);
        assert!(parse(&["mosh", "--port", "60001", "h"], &env).is_err());
    }

    #[test]
    fn missing_host_errors() {
        let env = env_of(&[("HOME", "/h")]);
        assert!(parse(&["mosh"], &env).is_err());
    }
}

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

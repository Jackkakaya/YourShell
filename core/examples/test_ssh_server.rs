//! Throwaway SSH echo server for verifying the `ssh` builtin end-to-end — NOT
//! shipped, host-only. Accepts password "testpw", opens a shell that greets
//! with a banner and echoes input; sending a line containing "bye" makes it
//! send GOODBYE and close with exit status 0.
//!
//!   cargo run --release --example test_ssh_server -- 127.0.0.1:2222

use std::sync::Arc;

use russh::keys::ssh_key::PublicKey;
use russh::keys::PrivateKey;
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use tokio::net::TcpListener;

/// Throwaway ed25519 host key (generated once for this test fixture only).
const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACASUohKfMuQ2nSEjXhqyzuY9wqL0U7Q/bDByT/cJIhkwwAAAJAZNYikGTWI
pAAAAAtzc2gtZWQyNTUxOQAAACASUohKfMuQ2nSEjXhqyzuY9wqL0U7Q/bDByT/cJIhkww
AAAEAv8BDRB99pH38H1yBQMD/6JeyEcwAdp6tOHhhbzOp7JxJSiEp8y5DadISNeGrLO5j3
CovRTtD9sMHJP9wkiGTDAAAACXRocm93YXdheQECAwQ=
-----END OPENSSH PRIVATE KEY-----
";

#[derive(Clone)]
struct Srv;

impl server::Server for Srv {
    type Handler = Handler;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Handler {
        Handler
    }
}

struct Handler;

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        if password == "testpw" {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    async fn auth_publickey(&mut self, _user: &str, _key: &PublicKey) -> Result<Auth, Self::Error> {
        // Reject keys so the client falls through to password auth.
        Ok(Auth::Reject {
            proceed_with_methods: None,
            partial_success: false,
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, b"WELCOME-YOURSHELL\r\n".to_vec())?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Echo everything back (like a PTY would).
        session.data(channel, data.to_vec())?;
        if data.windows(3).any(|w| w == b"bye") {
            session.data(channel, b"\r\nGOODBYE\r\n".to_vec())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:2222".into());
    let config = server::Config {
        keys: vec![PrivateKey::from_openssh(HOST_KEY).unwrap()],
        ..Default::default()
    };
    let listener = TcpListener::bind(&addr).await.unwrap();
    eprintln!("test_ssh_server listening on {addr} (password: testpw)");
    let mut srv = Srv;
    srv.run_on_socket(Arc::new(config), &listener)
        .await
        .unwrap();
}

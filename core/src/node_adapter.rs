//! Bridges Node.js (nodejs-mobile) into brush as the `node`/`npm`/`npx`
//! builtins.
//!
//! node_start() can only run once per process, so the app launches one
//! resident Node instance at startup (node_host.c + main.js dispatcher). This
//! adapter never starts Node; each command connects to the resident instance
//! over loopback TCP, sends a JSON request (argv/cwd/env/stdin), and streams
//! the framed stdout/stderr back to the session's fds. Because Node runs in
//! its own instance, this needs neither the process-state lock nor fd dup2 —
//! output is written directly to the session's OpenFile.
//!
//! The request carries a shared secret (`YS_NODE_TOKEN`): iOS does not isolate
//! loopback between apps, so an unauthenticated dispatcher would let any other
//! app on the device execute Node code inside our sandbox.
//!
//! Compiled only with the `node` cargo feature.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

/// Absolute path to npm's cli.js inside the bundle; set by the app before the
/// core starts. `npm`/`npx` rewrite to `node <cli.js>`.
fn npm_cli() -> Option<PathBuf> {
    std::env::var_os("YS_NODE_NPM_CLI").map(PathBuf::from)
}
fn npx_cli() -> Option<PathBuf> {
    std::env::var_os("YS_NODE_NPX_CLI").map(PathBuf::from)
}

unsafe extern "C" {
    /// Provided by node_host.c: starts the resident Node instance (idempotent).
    fn ys_node_start_resident(main_js_path: *const std::ffi::c_char);
}

/// Ensures the resident Node instance is starting. Idempotent on the C side,
/// so calling it on every node command is fine — only the first one launches.
fn ensure_node_started() {
    if let Some(main_js) = std::env::var_os("YS_NODE_MAIN_JS") {
        if let Ok(c) = std::ffi::CString::new(main_js.to_string_lossy().into_owned()) {
            unsafe { ys_node_start_resident(c.as_ptr()) };
        }
    }
}

/// Reads the resident instance's port (written by main.js). Retries while Node
/// starts up (the first command triggers a lazy launch that takes ~1-2s).
fn resident_port() -> Option<u16> {
    ensure_node_started();
    let file = std::env::var_os("YS_NODE_PORT_FILE")?;
    for _ in 0..100 {
        if let Ok(s) = std::fs::read_to_string(&file) {
            if let Ok(p) = s.trim().parse::<u16>() {
                if p != 0 {
                    return Some(p);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec_node,
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
    Ok(format!(
        "{name}: Node.js 18 (nodejs-mobile, resident instance)"
    ))
}

fn exec_node(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let name = context.command_name.clone();
        let mut argv: Vec<String> = args.iter().map(ToString::to_string).collect();

        // Rewrite npm/npx to `node <cli.js> <args>`.
        match name.as_str() {
            "npm" => {
                let Some(cli) = npm_cli() else {
                    writeln!(context.stderr(), "npm: not bundled")?;
                    return Ok(ExecutionResult::new(127));
                };
                let rest = argv.split_off(1);
                argv = vec!["node".into(), cli.to_string_lossy().into_owned()];
                argv.extend(rest);
            }
            "npx" => {
                let Some(cli) = npx_cli() else {
                    writeln!(context.stderr(), "npx: not bundled")?;
                    return Ok(ExecutionResult::new(127));
                };
                let rest = argv.split_off(1);
                argv = vec!["node".into(), cli.to_string_lossy().into_owned()];
                argv.extend(rest);
            }
            _ => {}
        }

        let cwd = context.shell.working_dir().to_string_lossy().into_owned();
        let env: Vec<(String, String)> = context
            .shell
            .env()
            .iter_exported()
            .filter(|(_, v)| v.value().is_set())
            .map(|(k, v)| (k.clone(), v.value().to_cow_str(context.shell).into_owned()))
            .collect();

        // Drain redirected/piped stdin to forward to node. The session's base
        // stdin is a pipe (the keyboard), so is_terminal() can't distinguish
        // interactive from redirected — use brush's authoritative "was fd 0
        // specified?" (matching the awk/python/uutils adapters). Draining the
        // live keyboard would block forever on an EOF that never comes; a
        // dev+inode fd comparison is unreliable for pipes.
        let stdin_is_interactive = !context.params.is_fd_specified(0.into());
        let cmd_stdin = context.try_fd(0.into()).and_then(|f| {
            f.try_borrow_as_fd()
                .ok()
                .and_then(|b| b.try_clone_to_owned().ok())
        });
        let mut stdin_data = Vec::new();
        if !stdin_is_interactive {
            if let Some(fd) = cmd_stdin {
                let _ = std::fs::File::from(fd).read_to_end(&mut stdin_data);
            }
        }

        let request = build_request(&argv, &cwd, &env, &stdin_data);

        let mut out = context.stdout();
        let mut err = context.stderr();
        let code =
            tokio::task::spawn_blocking(move || run_via_resident(&request, &mut out, &mut err))
                .await
                .unwrap_or(126);

        #[expect(clippy::cast_sign_loss)]
        Ok(ExecutionResult::new((code & 0xff) as u8))
    })
}

fn build_request(argv: &[String], cwd: &str, env: &[(String, String)], stdin: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    // Requests are newline-delimited JSON, so any control chars in argv/env
    // (a value containing a literal newline) must be escaped or they'd split
    // the frame and break parsing on the node side.
    let esc = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    };
    let argv_json: Vec<String> = argv.iter().map(|a| format!("\"{}\"", esc(a))).collect();
    let env_json: Vec<String> = env
        .iter()
        .map(|(k, v)| format!("\"{}\":\"{}\"", esc(k), esc(v)))
        .collect();
    // Shared secret proving we are the host that launched this Node instance.
    // iOS loopback is not isolated between apps, so the dispatcher refuses any
    // request without it (see the auth block in main.js). The host sets it once
    // per launch, before either the core or Node starts.
    let token = std::env::var("YS_NODE_TOKEN").unwrap_or_default();
    format!(
        "{{\"tok\":\"{}\",\"argv\":[{}],\"cwd\":\"{}\",\"env\":{{{}}},\"stdin\":\"{}\"}}\n",
        esc(&token),
        argv_json.join(","),
        esc(cwd),
        env_json.join(","),
        b64.encode(stdin),
    )
}

fn run_via_resident(request: &str, out: &mut impl Write, err: &mut impl Write) -> i32 {
    let Some(port) = resident_port() else {
        let _ = writeln!(err, "node: resident instance not available");
        return 125;
    };
    let stream = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "node: cannot reach instance: {e}");
            return 125;
        }
    };
    if stream
        .set_read_timeout(Some(Duration::from_secs(600)))
        .is_err()
    {
        // best effort
    }
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(e) => {
            let _ = writeln!(err, "node: {e}");
            return 125;
        }
    };
    if writer.write_all(request.as_bytes()).is_err() {
        let _ = writeln!(err, "node: failed to send request");
        return 125;
    }
    let _ = writer.flush();

    let b64 = base64::engine::general_purpose::STANDARD;
    let reader = BufReader::new(stream);
    let mut exit_code = 0;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.is_empty() {
            continue;
        }
        // Minimal JSON frame parsing: {"o":"..."} / {"e":"..."} / {"exit":N}.
        if let Some(payload) = frame_str(&line, "\"o\":\"") {
            if let Ok(bytes) = b64.decode(payload) {
                let _ = out.write_all(&bytes);
            }
        } else if let Some(payload) = frame_str(&line, "\"e\":\"") {
            if let Ok(bytes) = b64.decode(payload) {
                let _ = err.write_all(&bytes);
            }
        } else if let Some(code) = frame_int(&line, "\"exit\":") {
            exit_code = code;
        }
    }
    let _ = out.flush();
    let _ = err.flush();
    exit_code
}

fn frame_str<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line.find(key)? + key.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn frame_int(line: &str, key: &str) -> Option<i32> {
    let start = line.find(key)? + key.len();
    let rest = &line[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

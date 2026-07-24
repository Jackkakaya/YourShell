'use strict';
// Resident Node.js dispatcher for YourShell.
//
// node_start() can only run once per process, so this script is launched once
// at app startup and never exits. It listens on a loopback TCP port; the Rust
// core connects per command and sends a JSON request describing the command
// to run. We execute it inside this same instance (npm/npx are just Node CLIs,
// user scripts run via the module loader), capturing stdout/stderr and
// streaming the bytes back framed on the same socket, then a final exit code.
//
// Protocol (newline-delimited JSON, then raw framed output):
//   client -> server: {"argv":[...], "cwd":"...", "env":{...}, "stdin":"..."}\n
//   server -> client: frames of  {"o":<base64 stdout>} / {"e":<base64 stderr>}
//                     terminated by {"exit":<code>}\n
//
// The port is written to $YS_NODE_PORT_FILE once listening so the Rust side
// can discover it without a fixed port.

const net = require('net');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const Module = require('module');

const PORT_FILE = process.env.YS_NODE_PORT_FILE;

function b64(buf) {
  return Buffer.from(buf).toString('base64');
}

function runRequest(req, sock) {
  // Per-command captured streams -> framed back to the client.
  const send = (obj) => {
    try {
      sock.write(JSON.stringify(obj) + '\n');
    } catch (_) {}
  };
  const writeOut = (chunk, enc, cb) => {
    send({ o: b64(chunk) });
    if (cb) cb();
    return true;
  };
  const writeErr = (chunk, enc, cb) => {
    send({ e: b64(chunk) });
    if (cb) cb();
    return true;
  };

  const argv = req.argv || [];
  const cwd = req.cwd || process.cwd();
  try {
    process.chdir(cwd);
  } catch (_) {}
  if (req.env) {
    for (const k of Object.keys(req.env)) process.env[k] = req.env[k];
  }

  // Swap the real stdout/stderr write for the duration of this command.
  const realOut = process.stdout.write.bind(process.stdout);
  const realErr = process.stderr.write.bind(process.stderr);
  process.stdout.write = writeOut;
  process.stderr.write = writeErr;

  let exitCode = 0;
  const origExit = process.exit;
  const ExitSignal = {};
  process.exit = (code) => {
    exitCode = code || 0;
    throw ExitSignal;
  };

  try {
    // Evaluate a code string with node's usual module scope (require, module,
    // __dirname, console, process all in scope), rooted at cwd.
    const evalInModule = (code, wantResult) => {
      const m = new Module('[eval]', null);
      m.filename = path.join(cwd, '[eval]');
      m.paths = Module._nodeModulePaths(cwd);
      const wrapped = wantResult
        ? 'module.exports = (' + code + ');'
        : code;
      m._compile(wrapped, m.filename);
      return m.exports;
    };

    // argv: [ "node", arg1, arg2, ... ]
    const args = argv.slice(1);
    if (args.length === 0) {
      send({ e: b64('node: interactive REPL not supported over dispatcher\n') });
      exitCode = 1;
    } else if (args[0] === '-e' || args[0] === '--eval') {
      const code = args[1] || '';
      process.argv = ['node', '-e', ...args.slice(2)];
      evalInModule(code, false);
    } else if (args[0] === '-v' || args[0] === '--version') {
      writeOut(process.version + '\n');
    } else if (args[0] === '-p' || args[0] === '--print') {
      const code = args[1] || '';
      const result = evalInModule(code, true);
      writeOut(require('util').inspect(result) + '\n');
    } else {
      // Run a script file (or npm/npx cli.js) in a fresh module scope.
      const scriptPath = path.resolve(cwd, args[0]);
      process.argv = ['node', scriptPath, ...args.slice(1)];
      const m = new Module(scriptPath, null);
      m.filename = scriptPath;
      m.paths = Module._nodeModulePaths(path.dirname(scriptPath));
      const src = fs.readFileSync(scriptPath, 'utf8');
      m._compile(src, scriptPath);
    }
  } catch (err) {
    if (err !== ExitSignal) {
      writeErr((err && err.stack ? err.stack : String(err)) + '\n');
      exitCode = exitCode || 1;
    }
  } finally {
    process.stdout.write = realOut;
    process.stderr.write = realErr;
    process.exit = origExit;
  }

  // Allow queued async output a tick to flush, then close.
  setImmediate(() => {
    send({ exit: exitCode });
    sock.end();
  });
}

const server = net.createServer((sock) => {
  let buf = '';
  let handled = false;
  sock.on('data', (d) => {
    if (handled) return;
    buf += d.toString('utf8');
    const nl = buf.indexOf('\n');
    if (nl >= 0) {
      handled = true;
      const line = buf.slice(0, nl);
      try {
        runRequest(JSON.parse(line), sock);
      } catch (e) {
        sock.write(JSON.stringify({ e: b64('dispatch error: ' + e + '\n') }) + '\n');
        sock.write(JSON.stringify({ exit: 1 }) + '\n');
        sock.end();
      }
    }
  });
});

server.listen(0, '127.0.0.1', () => {
  const port = server.address().port;
  if (PORT_FILE) {
    fs.writeFileSync(PORT_FILE, String(port));
  }
  // Keep the event loop alive forever.
});

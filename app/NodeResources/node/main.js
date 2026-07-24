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

// libuv's uv_set_process_title crashes on iOS (it pokes argv/sysctl memory),
// and npm/yarn set process.title. Make it an inert property up front.
try {
  let _title = 'node';
  Object.defineProperty(process, 'title', {
    get() { return _title; },
    set(v) { _title = String(v); },
    configurable: true,
  });
} catch (_) {}

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
  let lastWriteAt = Date.now();
  const writeOut = (chunk, enc, cb) => {
    lastWriteAt = Date.now();
    send({ o: b64(chunk) });
    if (typeof enc === 'function') enc();
    else if (cb) cb();
    return true;
  };
  const writeErr = (chunk, enc, cb) => {
    lastWriteAt = Date.now();
    send({ e: b64(chunk) });
    if (typeof enc === 'function') enc();
    else if (cb) cb();
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
  let exited = false;
  let isEvalMode = false;
  const origExit = process.exit;
  // Record the exit code but do NOT throw: npm and other CLIs call
  // process.exit() from async contexts, and throwing across node's internal
  // async boundaries corrupts the runtime. We just remember the code and let
  // the command settle, reporting it when output flushes.
  process.exit = (code) => {
    if (code !== undefined && code !== null) exitCode = code;
    exited = true;
  };
  const origExitCodeDesc = Object.getOwnPropertyDescriptor(process, 'exitCode');

  try {
    // Evaluate a code string with node's usual module scope. require is bound
    // to cwd via the official createRequire so `require('installed-pkg')`
    // resolves cwd/node_modules — matching `node -e` run from that directory.
    const evalInModule = (code, wantResult) => {
      const anchor = path.join(cwd, '[eval].js');
      const req = Module.createRequire(anchor);
      const mod = { exports: {} };
      const fn = vm.compileFunction(
        wantResult ? 'return (' + code + ');' : code,
        ['require', 'module', 'exports', '__dirname', '__filename'],
        { filename: anchor }
      );
      return fn(req, mod, mod.exports, cwd, anchor);
    };

    // argv: [ "node", arg1, arg2, ... ]
    const args = argv.slice(1);
    // Eval modes are synchronous; script/CLI modes (npm) run async and signal
    // completion via process.exit. This drives how we detect "done" below.
    isEvalMode = args[0] === '-e' || args[0] === '--eval'
      || args[0] === '-p' || args[0] === '--print'
      || args[0] === '-v' || args[0] === '--version';
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
    writeErr((err && err.stack ? err.stack : String(err)) + '\n');
    exitCode = exitCode || 1;
  }

  // Commands may finish asynchronously (npm, promises). Finish when exit() was
  // called and output has been quiet for a moment, or after a long idle with
  // no writes (covers commands that never call exit), or a hard ceiling.
  const startedAt = Date.now();
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    clearInterval(timer);
    process.stdout.write = realOut;
    process.stderr.write = realErr;
    process.exit = origExit;
    if (typeof process.exitCode === 'number') exitCode = process.exitCode;
    if (origExitCodeDesc) {
      try { process.exitCode = origExitCodeDesc.value; } catch (_) {}
    }
    send({ exit: exitCode });
    sock.end();
  };
  if (isEvalMode) {
    // Synchronous: finish after the current stack + a microtask/immediate for
    // any queued stdout to flush. Never wait on the idle heuristic.
    setImmediate(() => setImmediate(finish));
    return;
  }
  // CLI/script mode (npm, npx, user scripts): the authoritative "done" signal
  // is process.exit (npm always calls it). Fall back to a long idle for the
  // rare script that ends without exiting, and a hard ceiling.
  const timer = setInterval(() => {
    const idle = Date.now() - lastWriteAt;
    const elapsed = Date.now() - startedAt;
    if ((exited && idle >= 80) || idle >= 20000 || elapsed >= 600000) {
      finish();
    }
  }, 50);
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

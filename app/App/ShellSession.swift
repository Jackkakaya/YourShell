import Foundation

/// One shell session backed by a `brush_core::Shell` instance on a dedicated
/// Rust thread. Output bytes stream to `onOutput` (fed into SwiftTerm);
/// keyboard bytes come in through `keyInput`, which implements a minimal
/// line discipline: line editing + local echo while idle, raw forwarding to
/// the command's stdin while a command runs (enabling `read`, python REPL…).
final class ShellSession: ObservableObject {
    /// Terminal feed: raw output bytes (LF already mapped to CRLF).
    var onOutput: ((ArraySlice<UInt8>) -> Void)?

    @Published var cwd: String = ""
    private(set) var busy = false

    /// Mirror of everything fed to the terminal; used by the selftest/exec
    /// debug channels so the host can read results from a file.
    private(set) var transcript: String = ""

    private var handle: UnsafeMutableRawPointer?
    private var lineBuffer: [UInt8] = []
    private var history: [String] = []
    private var historyIndex: Int? = nil
    private var demoQueue: [String] = []
    var mirrorTranscript = false
    private var pendingEscape: [UInt8] = []

    init() {
        // Point the embedded CPython at the bundled stdlib before the Rust
        // core spins up any session. xcodegen folder resources land under
        // Resources/PythonResources/python.
        if let res = Bundle.main.resourcePath {
            setenv("YOURSHELL_PYTHON_HOME", res + "/PythonResources/python", 1)
        }
        // Writable site-packages for pip (the bundle is read-only and iOS
        // disables the user site); the python driver puts it on sys.path.
        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
        let pySite = library.appendingPathComponent("python/site-packages").path
        try? FileManager.default.createDirectory(
            atPath: pySite, withIntermediateDirectories: true)
        setenv("YOURSHELL_PY_SITE", pySite, 1)
        // Per-command env is handed to the persistent interpreter through this
        // file (its os.environ snapshot is frozen at init); the Rust adapter
        // writes it and the driver applies it each command.
        setenv("YS_PY_ENV_FILE",
               library.appendingPathComponent("py_env.json").path, 1)

        // Configure Node but don't start it — the resident instance launches
        // lazily on the first node/npm command (the Rust adapter calls
        // ys_node_start_resident). Setting env is cheap; starting Node costs
        // ~40MB, so users who never touch node pay nothing.
        ShellSession.configureNode()

        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        cwd = docs.path
        seedDemoFiles(in: docs)

        let ctx = Unmanaged.passUnretained(self).toOpaque()
        handle = ashell_session_new(
            { ctx, bytes, len in
                guard let ctx, let bytes, len > 0 else { return }
                let session = Unmanaged<ShellSession>.fromOpaque(ctx).takeUnretainedValue()
                let data = Array(UnsafeBufferPointer(start: bytes, count: len))
                DispatchQueue.main.async { session.emitPipeOutput(data) }
            },
            { ctx, exitCode, cwdPtr in
                guard let ctx else { return }
                let session = Unmanaged<ShellSession>.fromOpaque(ctx).takeUnretainedValue()
                let newCwd = cwdPtr.map { String(cString: $0) } ?? ""
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
                    if !newCwd.isEmpty { session.cwd = newCwd }
                    if exitCode != 0 { session.emitText("[exit \(exitCode)]\r\n") }
                    session.busy = false
                    session.printPrompt()
                    if session.mirrorTranscript {
                        let docs = FileManager.default.urls(
                            for: .documentDirectory, in: .userDomainMask)[0]
                        try? session.transcript.write(
                            to: docs.appendingPathComponent("exec_out.txt"),
                            atomically: true, encoding: .utf8)
                    }
                    session.runNextDemoCommand()
                }
            },
            ctx,
            docs.path
        )
    }

    /// Sets the env the Rust node adapter needs (main.js path, port file, npm
    /// CLI paths and config). Does not start Node — that happens lazily in the
    /// adapter via ys_node_start_resident on the first node/npm command.
    static func configureNode() {
        guard let res = Bundle.main.resourcePath else { return }
        let mainJS = res + "/NodeResources/node/main.js"
        guard FileManager.default.fileExists(atPath: mainJS) else { return }
        setenv("YS_NODE_MAIN_JS", mainJS, 1)

        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
        let portFile = library.appendingPathComponent("node_port.txt").path
        try? FileManager.default.removeItem(atPath: portFile)
        setenv("YS_NODE_PORT_FILE", portFile, 1)
        // npm/npx CLIs bundled alongside main.js.
        setenv("YS_NODE_NPM_CLI", res + "/NodeResources/node/npm/bin/npm-cli.js", 1)
        setenv("YS_NODE_NPX_CLI", res + "/NodeResources/node/npm/bin/npx-cli.js", 1)
        // npm needs writable cache/prefix; the bundle is read-only. Point them
        // into the app's Library, and default installs to ignore lifecycle
        // scripts (they'd spawn subprocesses, unsupported on iOS).
        let npmCache = library.appendingPathComponent("npm-cache").path
        let npmPrefix = library.appendingPathComponent("npm-global").path
        try? FileManager.default.createDirectory(atPath: npmCache, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(atPath: npmPrefix, withIntermediateDirectories: true)
        setenv("npm_config_cache", npmCache, 1)
        setenv("npm_config_prefix", npmPrefix, 1)
        setenv("npm_config_ignore_scripts", "true", 1)
        setenv("npm_config_fund", "false", 1)
        setenv("npm_config_audit", "false", 1)
        setenv("npm_config_update_notifier", "false", 1)
        // npm install is network-latency bound (dependency-tree resolution =
        // one registry round-trip per package; ~86% of install time). The
        // default registry.npmjs.org is far/slow here; a mirror cuts installs
        // ~4-5x. Users can override with `npm config` / npm_config_registry.
        if getenv("npm_config_registry") == nil {
            setenv("npm_config_registry", "https://registry.npmmirror.com", 1)
        }
        if getenv("HOME") == nil {
            setenv("HOME", library.deletingLastPathComponent().path, 1)
        }
    }

    var promptPathComponent: String {
        (cwd as NSString).lastPathComponent
    }

    // MARK: terminal output

    /// Pipe bytes use bare LF; terminals need CRLF.
    private func emitPipeOutput(_ bytes: [UInt8]) {
        var mapped: [UInt8] = []
        mapped.reserveCapacity(bytes.count + 16)
        var previous: UInt8 = 0
        for b in bytes {
            if b == 0x0A && previous != 0x0D {
                mapped.append(0x0D)
            }
            mapped.append(b)
            previous = b
        }
        emitBytes(mapped)
    }

    private func emitText(_ s: String) {
        emitBytes(Array(s.utf8))
    }

    private func emitBytes(_ bytes: [UInt8]) {
        transcript += String(decoding: bytes, as: UTF8.self)
        onOutput?(bytes[...])
    }

    func printPrompt() {
        emitText("\u{1B}[1;32m\(promptPathComponent) $\u{1B}[0m ")
    }

    // MARK: keyboard input

    func keyInput(_ bytes: ArraySlice<UInt8>) {
        if busy {
            // Raw mode: the running command owns stdin. Map CR to LF for
            // canonical line-based readers.
            var raw = Array(bytes)
            for i in raw.indices where raw[i] == 0x0D { raw[i] = 0x0A }
            // Local echo for line-based interactive programs (python REPL,
            // read): show what's typed, since the program can't echo.
            emitBytes(raw.map { $0 == 0x0A ? nil : $0 }.compactMap { $0 })
            if raw.contains(0x0A) { emitText("\r\n") }
            if let handle {
                raw.withUnsafeBufferPointer { buf in
                    ashell_stdin_write(handle, buf.baseAddress, buf.count)
                }
            }
            return
        }
        for b in bytes {
            handleEditingKey(b)
        }
    }

    private func handleEditingKey(_ b: UInt8) {
        // Arrow keys arrive as ESC [ A/B — track a tiny escape state.
        if !pendingEscape.isEmpty {
            pendingEscape.append(b)
            if pendingEscape.count == 3 {
                let key = pendingEscape[2]
                pendingEscape = []
                if key == 0x41 { recallHistory(direction: -1) }      // Up
                else if key == 0x42 { recallHistory(direction: 1) }  // Down
            }
            return
        }
        switch b {
        case 0x1B:
            pendingEscape = [b]
        case 0x0D, 0x0A: // Enter
            emitText("\r\n")
            let line = String(decoding: lineBuffer, as: UTF8.self)
            lineBuffer = []
            historyIndex = nil
            submit(line)
        case 0x7F, 0x08: // Backspace
            if !lineBuffer.isEmpty {
                // Pop one UTF-8 scalar (continuation bytes included).
                while let last = lineBuffer.last, (last & 0xC0) == 0x80 {
                    lineBuffer.removeLast()
                }
                if !lineBuffer.isEmpty { lineBuffer.removeLast() }
                emitText("\u{08} \u{08}")
            }
        case 0x15: // Ctrl-U: kill line
            eraseCurrentLine()
            lineBuffer = []
        case 0x03: // Ctrl-C at prompt: abandon line
            emitText("^C\r\n")
            lineBuffer = []
            printPrompt()
        default:
            if b >= 0x20 || b == 0x09 {
                lineBuffer.append(b)
                emitBytes([b])
            }
        }
    }

    private func eraseCurrentLine() {
        let scalars = String(decoding: lineBuffer, as: UTF8.self).count
        for _ in 0..<scalars { emitText("\u{08} \u{08}") }
    }

    private func recallHistory(direction: Int) {
        guard !history.isEmpty else { return }
        var idx = historyIndex ?? history.count
        idx = max(0, min(history.count, idx + direction))
        eraseCurrentLine()
        if idx == history.count {
            lineBuffer = []
            historyIndex = nil
        } else {
            let entry = history[idx]
            lineBuffer = Array(entry.utf8)
            historyIndex = idx
            emitText(entry)
        }
    }

    private func submit(_ line: String) {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            printPrompt()
            return
        }
        history.append(trimmed)
        busy = true
        if let handle {
            ashell_exec(handle, trimmed)
        }
    }

    /// Programmatic execution (demo/debug channels): echoes like a typed line.
    func run(_ command: String) {
        let trimmed = command.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let handle else { return }
        emitText("\(trimmed)\r\n")
        history.append(trimmed)
        busy = true
        ashell_exec(handle, trimmed)
    }

    // MARK: debug channels

    /// Runs the full command battery in-process and writes the report to
    /// Documents/selftest_report.txt so it can be pulled from the host.
    func runSelftest() {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        // Incremental report: the Rust battery flushes after each case, so
        // progress survives an iOS background kill on a long run.
        setenv("YS_SELFTEST_REPORT",
               docs.appendingPathComponent("selftest_report.txt").path, 1)
        emitText("running selftest battery ...\r\n")
        DispatchQueue.global(qos: .userInitiated).async {
            guard let raw = ashell_selftest(docs.path) else { return }
            let report = String(cString: raw)
            ashell_string_free(raw)
            try? report.write(
                to: docs.appendingPathComponent("selftest_report.txt"),
                atomically: true, encoding: .utf8)
            DispatchQueue.main.async { self.emitPipeOutput(Array(report.utf8)) }
        }
    }

    /// Debug channel: run one command, mirror the transcript to a file so the
    /// host can read it (`ASHELL_EXEC` env, output in Documents/exec_out.txt).
    func runSingle(_ cmd: String) {
        mirrorTranscript = true
        run(cmd)
    }

    /// Debug channel: feed bytes to the running command's stdin after a delay
    /// (`ASHELL_STDIN_FEED` env) — lets headless tests drive interactive
    /// programs like the python REPL.
    func scheduleStdinFeed(_ text: String, after seconds: Double) {
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak self] in
            guard let self, let handle = self.handle else { return }
            let bytes = Array(text.utf8)
            bytes.withUnsafeBufferPointer { buf in
                ashell_stdin_write(handle, buf.baseAddress, buf.count)
            }
        }
    }

    func startDemo() {
        demoQueue = [
            "uname",
            "echo YourShell on brush-core, pid $$",
            "ls",
            "cat poem.txt | grep -n fork",
            "python3 -c 'print(\"python\", 6*7)'",
            "seq 3 | sort -r | paste -sd, -",
        ]
        runNextDemoCommand()
    }

    fileprivate func runNextDemoCommand() {
        guard !demoQueue.isEmpty else { return }
        run(demoQueue.removeFirst())
    }

    private func seedDemoFiles(in dir: URL) {
        let poem = dir.appendingPathComponent("poem.txt")
        if !FileManager.default.fileExists(atPath: poem.path) {
            let text = """
            rust core, swift shell,
            no fork, no exec — and yet
            pipes still carry words.
            """
            try? text.write(to: poem, atomically: true, encoding: .utf8)
        }
    }

    deinit {
        if let handle { ashell_session_free(handle) }
    }
}

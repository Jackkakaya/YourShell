import Foundation

/// One shell session backed by a `brush_core::Shell` instance living on a
/// dedicated Rust thread. Output arrives on a Rust reader thread and is
/// forwarded to the main actor.
final class ShellSession: ObservableObject {
    @Published var transcript: String = ""
    @Published var cwd: String = ""
    @Published var busy: Bool = false

    private var handle: UnsafeMutableRawPointer?
    private var demoQueue: [String] = []

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
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        cwd = docs.path
        seedDemoFiles(in: docs)

        let ctx = Unmanaged.passUnretained(self).toOpaque()
        handle = ashell_session_new(
            { ctx, bytes, len in
                guard let ctx, let bytes, len > 0 else { return }
                let session = Unmanaged<ShellSession>.fromOpaque(ctx).takeUnretainedValue()
                let data = Data(bytes: bytes, count: len)
                let text = String(decoding: data, as: UTF8.self)
                DispatchQueue.main.async { session.transcript += text }
            },
            { ctx, exitCode, cwdPtr in
                guard let ctx else { return }
                let session = Unmanaged<ShellSession>.fromOpaque(ctx).takeUnretainedValue()
                let newCwd = cwdPtr.map { String(cString: $0) } ?? ""
                // The output pipe is drained on a separate thread; give the
                // last chunk a beat to land before we mark the command done.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
                    if !newCwd.isEmpty { session.cwd = newCwd }
                    if exitCode != 0 { session.appendLine("[exit \(exitCode)]") }
                    session.busy = false
                    session.runNextDemoCommand()
                }
            },
            ctx,
            docs.path
        )
    }

    var promptPathComponent: String {
        (cwd as NSString).lastPathComponent
    }

    func run(_ command: String) {
        let trimmed = command.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let handle else { return }
        appendLine("\(promptPathComponent) $ \(trimmed)")
        busy = true
        ashell_exec(handle, trimmed)
    }

    fileprivate func appendLine(_ line: String) {
        if !transcript.isEmpty && !transcript.hasSuffix("\n") {
            transcript += "\n"
        }
        transcript += line + "\n"
    }

    /// Runs the full command battery in-process and writes the report to
    /// Documents/selftest_report.txt so it can be pulled from the host.
    func runSelftest() {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        transcript += "running selftest battery (\("all commands")) ...\n"
        DispatchQueue.global(qos: .userInitiated).async {
            guard let raw = ashell_selftest(docs.path) else { return }
            let report = String(cString: raw)
            ashell_string_free(raw)
            try? report.write(
                to: docs.appendingPathComponent("selftest_report.txt"),
                atomically: true, encoding: .utf8)
            DispatchQueue.main.async { self.transcript += report }
        }
    }

    /// Debug channel: run one command, mirror the transcript to a file so the
    /// host can read it (`ASHELL_EXEC` env, output in Documents/exec_out.txt).
    func runSingle(_ cmd: String) {
        run(cmd)
        DispatchQueue.main.asyncAfter(deadline: .now() + 30) { [weak self] in
            guard let self else { return }
            let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            try? self.transcript.write(
                to: docs.appendingPathComponent("exec_out.txt"),
                atomically: true, encoding: .utf8)
        }
    }

    func startDemo() {
        demoQueue = [
            "uname -a",
            "echo hello from brush-core on iOS, pid $$",
            "ls",
            "cat poem.txt",
            "cat poem.txt | wc -l",
            "for i in 1 2 3; do echo \"loop $i: $((i * i))\"; done",
            "x=42; if [ $x -gt 10 ]; then echo \"branching works: x=$x\"; fi",
            "mkdir -p sub && cd sub && pwd && cd ..",
            "type ls; type cd",
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

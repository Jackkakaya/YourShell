import SwiftUI
import SwiftTerm

@main
struct AShellApp: App {
    var body: some Scene {
        WindowGroup {
            TerminalScreen()
                .ignoresSafeArea(.container, edges: .bottom)
                .background(Color.black)
        }
    }
}

struct TerminalScreen: View {
    @StateObject private var session = ShellSession()

    var body: some View {
        TerminalHostView(session: session)
            .onAppear {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    session.printPrompt()
                    let env = ProcessInfo.processInfo.environment
                    if env["ASHELL_SELFTEST"] == "1" {
                        session.runSelftest()
                    } else if let cmd = env["ASHELL_EXEC"] {
                        session.runSingle(cmd)
                        if let feed = env["ASHELL_STDIN_FEED"] {
                            session.scheduleStdinFeed(feed, after: 4.0)
                        }
                    } else if let typed = env["ASHELL_TYPE"] {
                        // Debug: feed keystrokes (incl. \t) into the line editor,
                        // then mirror the transcript so the host can inspect it.
                        session.typeForDebug(typed)
                    } else {
                        session.startDemo()
                    }
                }
            }
    }
}

/// Hosts SwiftTerm's TerminalView: session output feeds the terminal,
/// terminal keystrokes feed the session's line discipline.
struct TerminalHostView: UIViewRepresentable {
    let session: ShellSession

    func makeUIView(context: Context) -> SwiftTerm.TerminalView {
        let view = SwiftTerm.TerminalView(frame: .zero)
        view.terminalDelegate = context.coordinator
        view.backgroundColor = .black
        view.nativeBackgroundColor = .black
        view.nativeForegroundColor = UIColor(
            red: 0.85, green: 0.95, blue: 0.85, alpha: 1.0)
        view.font = UIFont.monospacedSystemFont(ofSize: 13, weight: .regular)
        session.onOutput = { [weak view] bytes in
            view?.feed(byteArray: bytes)
        }
        _ = view.becomeFirstResponder()
        return view
    }

    func updateUIView(_ uiView: SwiftTerm.TerminalView, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(session: session)
    }

    final class Coordinator: NSObject, TerminalViewDelegate {
        let session: ShellSession
        init(session: ShellSession) { self.session = session }

        func send(source: SwiftTerm.TerminalView, data: ArraySlice<UInt8>) {
            DispatchQueue.main.async { self.session.keyInput(data) }
        }

        func sizeChanged(source: SwiftTerm.TerminalView, newCols: Int, newRows: Int) {}
        func setTerminalTitle(source: SwiftTerm.TerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: SwiftTerm.TerminalView, directory: String?) {}
        func scrolled(source: SwiftTerm.TerminalView, position: Double) {}
        func requestOpenLink(
            source: SwiftTerm.TerminalView, link: String, params: [String: String]) {}
        func clipboardCopy(source: SwiftTerm.TerminalView, content: Data) {}
        func rangeChanged(source: SwiftTerm.TerminalView, startY: Int, endY: Int) {}
    }
}

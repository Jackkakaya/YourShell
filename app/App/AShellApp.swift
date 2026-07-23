import SwiftUI

@main
struct AShellApp: App {
    var body: some Scene {
        WindowGroup {
            TerminalView()
        }
    }
}

struct TerminalView: View {
    @StateObject private var session = ShellSession()
    @State private var input = ""
    @FocusState private var inputFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    Text(session.transcript.isEmpty ? "aShell-rs — brush-core on iOS\n" : session.transcript)
                        .font(.system(size: 13, design: .monospaced))
                        .foregroundColor(Color(red: 0.85, green: 0.95, blue: 0.85))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                        .id("bottom")
                }
                .onChange(of: session.transcript) {
                    proxy.scrollTo("bottom", anchor: .bottom)
                }
            }
            .background(Color(red: 0.05, green: 0.07, blue: 0.05))

            HStack(spacing: 6) {
                Text("\(session.promptPathComponent) $")
                    .font(.system(size: 13, weight: .bold, design: .monospaced))
                    .foregroundColor(.green)
                TextField("command", text: $input)
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundColor(.white)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .focused($inputFocused)
                    .onSubmit {
                        session.run(input)
                        input = ""
                        inputFocused = true
                    }
                if session.busy {
                    ProgressView().scaleEffect(0.7)
                }
            }
            .padding(10)
            .background(Color(red: 0.1, green: 0.12, blue: 0.1))
        }
        .onAppear {
            inputFocused = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                if ProcessInfo.processInfo.environment["ASHELL_SELFTEST"] == "1" {
                    session.runSelftest()
                } else if let cmd = ProcessInfo.processInfo.environment["ASHELL_EXEC"] {
                    session.runSingle(cmd)
                } else {
                    session.startDemo()
                }
            }
        }
    }
}

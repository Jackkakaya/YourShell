# YourShell

A native iOS terminal: Rust core (brush-core) + Swift shell. bash syntax, common Linux commands, python3 + pip, node + npm — all in-process, no fork/exec, no JIT.

- `docs/DESIGN.md` — architecture & roadmap
- `core/` — Rust core (FFI sessions, in-process commands, 102-case selftest)
- `app/` — iOS app shell (SwiftUI, xcodegen)
- `vendor/brush` — brush with iOS patches (upstream PR reubeno/brush#1246)

## Build (simulator)

```sh
cd core && cargo build --release --target aarch64-apple-ios-sim
cd ../app && xcodegen generate && xcodebuild -project AShellRS.xcodeproj -scheme AShellRS -destination 'platform=iOS Simulator,name=iPhone 15 Pro' build
```

Tests: `cd core && cargo test --release` (host), or launch the app with `ASHELL_SELFTEST=1` (simulator).

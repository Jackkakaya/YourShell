# YourShell

YourShell is an iOS-native shell built around a Rust execution core and a
Swift terminal host. It supports Bash syntax, common Unix commands, Git,
Python, Node.js, SSH, SCP, SFTP, and Mosh without relying on `fork`, `exec`, or
JIT compilation.

> The project is under active development. Command compatibility is tested
> continuously, but it is not yet a drop-in replacement for a desktop Linux
> environment.

## Highlights

- Bash parsing and builtins powered by [Brush](https://github.com/reubeno/brush).
- 166 registered commands and 365 functional command scenarios.
- Upstream CLI parsers reused where practical: uutils/coreutils, ripgrep,
  uutils grep/find/sed/diff, curl, libarchive tools, SQLite, and jq.
- In-process Git implementation with clone, status, add, commit, log, diff,
  branch, remote, push, pull, rebase, stash, cherry-pick, and revert coverage.
- Embedded Python and Node.js runtimes designed for iOS restrictions.
- Real localhost protocol tests for SSH, SCP, and SFTP.
- Session-scoped cwd, environment, stdio, cancellation, and terminal ownership.

## Architecture

```text
SwiftUI + SwiftTerm
        |
        | C ABI callbacks: input, output, completion, iOS hosts
        v
YourShell session (Rust)
        |
        +-- Brush parser and Bash builtins
        +-- process-shaped CLI adapters
        +-- session-safe Rust commands
        +-- Python / Node runtime hosts
        +-- Git / SSH / SCP / SFTP / Mosh adapters
        |
        v
iOS filesystem, networking, clipboard, URLs, camera and OCR
```

The command integration rule is deliberately simple:

1. Reuse a mature upstream CLI parser and implementation when it can run
   in-process.
2. Keep the YourShell adapter limited to argv, cwd, env, stdio, cancellation,
   and iOS host integration.
3. Implement locally only when iOS lifecycle or session mutation requires it.

See [Command integration](docs/research/COMMAND_INTEGRATION.md) and the
[tool-system audit](docs/research/TOOL_SYSTEM_AUDIT.md) for the detailed
inventory and design history.

## Repository layout

| Path | Purpose |
| --- | --- |
| `core/` | Rust shell, FFI, command adapters, and tests |
| `app/` | Standalone iOS example app and Swift host |
| `vendor/brush/` | Brush submodule with iOS support |
| `vendor/python-ios-lib/` | Prebuilt iOS Python packages |
| `vendor/nodejs-mobile/` | NodeMobile XCFramework, stored with Git LFS |
| `tests/` | Runtime and repository-level tests |
| `scripts/` | Canonical build and verification commands |
| `docs/research/` | Command audits, compatibility research, and backlog |

## Requirements

- macOS with a current Xcode
- Rust stable toolchain
- Git LFS
- XcodeGen for the example app
- Python 3 for repository contract tests

Install the basic tools:

```sh
brew install git-lfs xcodegen
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
git lfs install
```

## Clone and bootstrap

The Brush source is a submodule, NodeMobile uses Git LFS, and the remaining
iOS runtime is a versioned, checksum-verified release artifact:

```sh
git clone --recurse-submodules https://github.com/Jackkakaya/YourShell.git
cd YourShell
./scripts/bootstrap-ios.sh
```

The bootstrap command is idempotent and prepares all Python and Node
frameworks and resources. It gives an actionable error when Git LFS or another
required tool is unavailable. For CI or an internal mirror, set
`YOURSHELL_CACHE_DIR` or `YOURSHELL_IOS_RUNTIME_URL`.

For an existing clone or a parent repository using YourShell as a submodule:

```sh
git submodule update --init --recursive
./vendor/YourShell/scripts/bootstrap-ios.sh
```

## Build and test the Rust core

The same command is used locally and in CI:

```sh
./scripts/ci.sh
```

Individual stages are also available:

```sh
./scripts/ci.sh fmt
./scripts/ci.sh check
./scripts/ci.sh test
./scripts/ci.sh contracts
```

The full Rust test suite includes:

- command inventory and functional battery;
- flag coverage for adapter-owned command interfaces;
- Git compatibility and state-transition scenarios;
- session capture, cancellation, and concurrency;
- iOS host callback tests;
- a temporary localhost OpenSSH server for real SSH/SCP/SFTP transfers.

## Build the iOS example app

```sh
./scripts/build-ios.sh sim
xcodegen generate --spec app/project.yml --project app
xcodebuild \
  -project app/AShellRS.xcodeproj \
  -scheme AShellRS \
  -configuration Debug \
  -destination 'generic/platform=iOS Simulator' \
  build \
  CODE_SIGNING_ALLOWED=NO
```

For a physical device, use `./scripts/build-ios.sh device`. The command
bootstraps every binary/resource dependency, installs the Rust target, and
produces the correct `libashellcore.a`. It is the supported integration entry
point for parent apps; consumers should not copy runtime files manually.

## Python runtime test on Simulator

Boot an iOS Simulator, then run:

```sh
./scripts/test_python_ios_lib_simulator.sh
```

This builds and installs the app, then tests imports and representative
operations for the bundled scientific and presentation packages through the
actual YourShell execution path.

## Compatibility expectations

- iOS does not permit arbitrary native executables, `fork`, or JIT runtimes.
- Commands must run in-process or through an explicit Swift host.
- Process-shaped upstream CLIs are serialized while their temporary cwd, env,
  and file descriptors are installed.
- Bash builtins such as `cd`, `export`, and `read` intentionally execute inside
  the current shell session because they mutate shell state.
- Python packages requiring an unavailable native iOS wheel cannot be built
  like they would be on a desktop system.

## Contributing

Start with [CONTRIBUTING.md](CONTRIBUTING.md). It explains command integration,
required tests, iOS constraints, commit expectations, and the pull-request
checklist.

Security reports should follow [SECURITY.md](SECURITY.md). Community behavior
is covered by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Project status

The current command and runtime backlog is maintained in
[docs/research/TODO.md](docs/research/TODO.md). Historical research is retained
for context, but passing tests and current code are authoritative when a
research document has become stale.

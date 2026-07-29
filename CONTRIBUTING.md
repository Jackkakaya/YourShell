# Contributing to YourShell

Thank you for helping make a capable shell fit naturally within iOS.

## Before you start

YourShell is not a conventional process-spawning shell. iOS forbids `fork`,
arbitrary executable loading, and JIT compilation. A contribution that works
on macOS but depends on those facilities is not complete.

For larger changes, open an issue first and describe:

- the user-visible behavior;
- the upstream implementation or protocol being reused;
- how it works without `fork` or `exec`;
- whether it mutates shell-session state;
- the simulator and device verification plan.

## Development setup

```sh
git clone --recurse-submodules https://github.com/Jackkakaya/YourShell.git
cd YourShell
git lfs install
git lfs pull
brew install xcodegen
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
./scripts/ci.sh
```

Do not commit generated Xcode projects, `DerivedData`, Cargo targets, local
Python bundles, or root-level `node_modules`.

## Command integration policy

Choose the smallest correct integration:

1. **Brush builtin** — for commands that must mutate the active shell, such as
   `cd`, `export`, `read`, aliases, jobs, and functions.
2. **Upstream Rust CLI** — preferred for process-shaped commands. Forward argv
   into the upstream parser instead of recreating flags with Clap.
3. **Vendored C CLI** — acceptable when its entry point can return normally
   and global cwd/env/stdio can be restored safely.
4. **Session-safe Rust command** — for implementations that operate directly
   on the session and do not install process-global state.
5. **Swift host or embedded runtime** — for iOS lifecycle, platform UI,
   Python, Node.js, camera, OCR, clipboard, and similar integrations.

Avoid partial command lookalikes. If an upstream parser exists, reuse it. If
full compatibility is impossible, document and test the supported interface.

## Tests required for command changes

Every registered command must remain represented in the command matrix.
Depending on the change, add tests at the appropriate seam:

- parser and error branches in the module's unit tests;
- command behavior in `core/src/selftest.rs`;
- advertised option coverage in `core/tests/flag_coverage.rs`;
- multi-step compatibility in a dedicated integration test;
- real local protocols when a temporary server can be isolated safely;
- Swift/iOS host behavior in simulator tests.

Run:

```sh
./scripts/ci.sh
```

For coverage work:

```sh
rustup toolchain install nightly
cargo install cargo-llvm-cov --locked
cd core
cargo +nightly llvm-cov --branch --all-targets --summary-only
```

Coverage must come from meaningful assertions. Do not exclude difficult
modules, mark reachable code as untestable, or add tests that only execute a
line without checking behavior.

## Rust and FFI rules

- Preserve the C ABI consumed by Swift unless the Swift declarations and
  compatibility checks change in the same pull request.
- Contain panics before they cross the C ABI.
- Restore temporary process cwd, env, signals, and file descriptors on every
  return path.
- Keep session-safe commands independent so separate sessions can run
  concurrently.
- Do not call `std::process::exit` from an in-process command.
- Keep unsafe blocks small and document their invariants.

## Documentation

Update README and contributor documentation when behavior, setup, or test
commands change. Research and implementation plans belong in `docs/research/`,
not at repository root.

The code and passing tests are authoritative. Label historical conclusions
clearly when they no longer describe current behavior.

## Pull requests

A pull request should:

- solve one coherent problem;
- explain the iOS constraints and compatibility boundary;
- include tests for success, failure, and repeated invocation;
- pass `./scripts/ci.sh`;
- avoid unrelated formatting or vendored-source changes;
- identify any generated or prebuilt artifacts and how to reproduce them.

Use clear commit messages. Keep authorship metadata accurate and do not add
automated co-author or session trailers.

## Review checklist

- [ ] No `fork`, `exec`, JIT, or unsupported dynamic loading
- [ ] Upstream parser reused where available
- [ ] Session mutation occurs in the active shell
- [ ] cwd/env/stdio restored on errors and panics
- [ ] Functional and error behavior tested
- [ ] Repeated invocation tested
- [ ] Simulator build considered
- [ ] README or research inventory updated

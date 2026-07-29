# Testing YourShell

## Canonical entry point

```sh
./scripts/ci.sh
```

This runs formatting, compilation checks, repository contracts, Clippy, and
the complete host test suite.

## Rust test groups

| Test | Purpose |
| --- | --- |
| library tests | Parsers, editor state, protocol codecs, runtime seams |
| `battery` | Bash behavior and command functional scenarios |
| `command_inventory` | Every registered command is present |
| `flag_coverage` | Adapter-advertised options have functional cases |
| `git_compat` | Multi-step Git state and remote workflows |
| `capture` | FFI output capture, timeout, and cancellation |
| `concurrency*` | Cross-session isolation and safe parallel commands |
| `ios_host` | Clipboard and URL callback adapters |
| `ssh_local` | Real SSH, SCP, and SFTP against an isolated local sshd |

## Repository contracts

`tests/repository_contract.py` checks documentation links, root-directory
hygiene, Git LFS declarations, and oversized regular Git blobs.

## iOS simulator

```sh
./scripts/test_python_ios_lib_simulator.sh
```

This is slower and requires a booted simulator. It validates the bundled
Python packages through the actual app and FFI path.

## Coverage

```sh
rustup toolchain install nightly
cargo install cargo-llvm-cov --locked
cd core
cargo +nightly llvm-cov --branch --all-targets --summary-only
```

Coverage is a guide to missing behavior, not a reason to weaken assertions or
exclude difficult production modules.

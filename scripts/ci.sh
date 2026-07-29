#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORE="$ROOT/core"
MODE="${1:-all}"

run_fmt() {
    cargo fmt --manifest-path "$CORE/Cargo.toml" --check
}

run_check() {
    cargo check --manifest-path "$CORE/Cargo.toml" --all-targets
}

run_contracts() {
    python3 -m unittest "$ROOT/tests/repository_contract.py"
}

run_clippy() {
    # Existing FFI-specific warnings remain visible but are not yet a hard
    # failure. Formatting, compilation, contracts and tests are hard gates.
    cargo clippy --manifest-path "$CORE/Cargo.toml" --all-targets -- \
        -A clippy::not_unsafe_ptr_arg_deref
}

run_tests() {
    cargo test --manifest-path "$CORE/Cargo.toml"
}

case "$MODE" in
    fmt) run_fmt ;;
    check) run_check ;;
    contracts) run_contracts ;;
    clippy) run_clippy ;;
    test) run_tests ;;
    all)
        run_fmt
        run_check
        run_contracts
        run_clippy
        run_tests
        ;;
    *)
        echo "usage: $0 [all|fmt|check|contracts|clippy|test]" >&2
        exit 2
        ;;
esac

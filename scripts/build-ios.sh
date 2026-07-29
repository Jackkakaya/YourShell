#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLATFORM="${1:-sim}"

case "$PLATFORM" in
    sim) TARGET="aarch64-apple-ios-sim" ;;
    device) TARGET="aarch64-apple-ios" ;;
    *)
        echo "usage: $0 [sim|device]" >&2
        exit 2
        ;;
esac

"$ROOT/scripts/bootstrap-ios.sh"

if ! command -v rustup >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
    echo "error: Rust is required; install it from https://rustup.rs" >&2
    exit 1
fi

rustup target add "$TARGET"
cargo build \
    --manifest-path "$ROOT/core/Cargo.toml" \
    --release \
    --target "$TARGET" \
    --features python,node

library="$ROOT/core/target/$TARGET/release/libashellcore.a"
[[ -s "$library" ]] || {
    echo "error: expected library was not produced: $library" >&2
    exit 1
}
echo "$library"

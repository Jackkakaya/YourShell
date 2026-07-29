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

DEPLOYMENT_TARGET="${YOURSHELL_IOS_DEPLOYMENT_TARGET:-17.0}"
FEATURES="${YOURSHELL_CARGO_FEATURES:-python,node}"
export IPHONEOS_DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET"

stamp="$ROOT/core/target/$TARGET/.yourshell-deployment-target"
if [[ ! -f "$stamp" ]] || [[ "$(<"$stamp")" != "$DEPLOYMENT_TARGET" ]]; then
    echo "Refreshing cached $TARGET objects for iOS $DEPLOYMENT_TARGET..."
    cargo clean \
        --manifest-path "$ROOT/core/Cargo.toml" \
        --target "$TARGET"
fi

rustup target add "$TARGET"
cargo build \
    --manifest-path "$ROOT/core/Cargo.toml" \
    --release \
    --target "$TARGET" \
    --features "$FEATURES"

library="$ROOT/core/target/$TARGET/release/libashellcore.a"
[[ -s "$library" ]] || {
    echo "error: expected library was not produced: $library" >&2
    exit 1
}
mkdir -p "$(dirname "$stamp")"
printf '%s\n' "$DEPLOYMENT_TARGET" >"$stamp"
echo "$library"

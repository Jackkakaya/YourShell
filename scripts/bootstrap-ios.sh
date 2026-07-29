#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=ios-runtime.env
source "$ROOT/scripts/ios-runtime.env"

CACHE_DIR="${YOURSHELL_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/yourshell}"
ARCHIVE="$CACHE_DIR/$YOURSHELL_IOS_RUNTIME_ARCHIVE"
CHECK_ONLY=0

usage() {
    cat <<'EOF'
Usage: ./scripts/bootstrap-ios.sh [--check]

Prepare every binary/resource dependency needed to embed YourShell on iOS.
The operation is idempotent. Set YOURSHELL_CACHE_DIR to share an artifact
cache, or YOURSHELL_IOS_RUNTIME_URL to use an internal artifact mirror.
EOF
}

for arg in "$@"; do
    case "$arg" in
        --check) CHECK_ONLY=1 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

required_paths=(
    "vendor/nodejs-mobile/NodeMobile.xcframework/Info.plist"
    "vendor/nodejs-mobile/NodeMobile.xcframework/ios-arm64/NodeMobile.framework/NodeMobile"
    "vendor/nodejs-mobile/NodeMobile.xcframework/ios-arm64_x86_64-simulator/NodeMobile.framework/NodeMobile"
    "vendor/python-ios/Python.xcframework/Info.plist"
    "app/PythonResources/python/lib/python3.14"
    "app/NodeResources/node/main.js"
    "app/NodeResources/node/npm/bin/npm-cli.js"
)
VERSION_MARKER="$ROOT/app/PythonResources/.yourshell-runtime-version"

missing_paths() {
    local path
    for path in "${required_paths[@]}"; do
        [[ -e "$ROOT/$path" ]] || printf '%s\n' "$path"
    done
    if [[ ! -f "$VERSION_MARKER" ]] ||
        [[ "$(<"$VERSION_MARKER")" != "$YOURSHELL_IOS_RUNTIME_VERSION" ]]; then
        printf '%s\n' "runtime version $YOURSHELL_IOS_RUNTIME_VERSION"
    fi
    for path in \
        "vendor/nodejs-mobile/NodeMobile.xcframework/ios-arm64/NodeMobile.framework/NodeMobile" \
        "vendor/nodejs-mobile/NodeMobile.xcframework/ios-arm64_x86_64-simulator/NodeMobile.framework/NodeMobile"; do
        if [[ -f "$ROOT/$path" ]] && [[ "$(wc -c <"$ROOT/$path")" -lt 1048576 ]]; then
            printf '%s\n' "$path (Git LFS content missing)"
        fi
    done
}

if [[ ! -f "$ROOT/core/Cargo.toml" ]]; then
    echo "error: YourShell source is incomplete (core/Cargo.toml is missing)." >&2
    echo "If this is a submodule, run: git submodule update --init --recursive" >&2
    exit 1
fi

missing="$(missing_paths)"
if [[ -z "$missing" ]]; then
    echo "YourShell iOS runtime is ready ($YOURSHELL_IOS_RUNTIME_VERSION)."
    exit 0
fi

if [[ "$CHECK_ONLY" -eq 1 ]]; then
    echo "error: YourShell iOS runtime is incomplete:" >&2
    printf '  %s\n' $missing >&2
    echo "Run: $ROOT/scripts/bootstrap-ios.sh" >&2
    exit 1
fi

if ! command -v git >/dev/null 2>&1; then
    echo "error: git is required." >&2
    exit 1
fi
if ! command -v curl >/dev/null 2>&1; then
    echo "error: curl is required." >&2
    exit 1
fi
if ! command -v shasum >/dev/null 2>&1; then
    echo "error: shasum is required." >&2
    exit 1
fi

if command -v git-lfs >/dev/null 2>&1; then
    git -C "$ROOT" lfs pull --include="vendor/nodejs-mobile/NodeMobile.xcframework/**"
else
    echo "error: Git LFS is required for NodeMobile (install with: brew install git-lfs)." >&2
    exit 1
fi

mkdir -p "$CACHE_DIR"
verify_archive() {
    [[ -f "$ARCHIVE" ]] &&
        [[ "$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')" == "$YOURSHELL_IOS_RUNTIME_SHA256" ]]
}

if ! verify_archive; then
    rm -f "$ARCHIVE"
    tmp_archive="$ARCHIVE.download.$$"
    trap 'rm -f "$tmp_archive"' EXIT
    echo "Downloading YourShell iOS runtime $YOURSHELL_IOS_RUNTIME_VERSION..."
    curl --fail --location --retry 5 --retry-all-errors \
        --connect-timeout 20 \
        --output "$tmp_archive" \
        "${YOURSHELL_IOS_RUNTIME_URL}"
    actual_sha="$(shasum -a 256 "$tmp_archive" | awk '{print $1}')"
    if [[ "$actual_sha" != "$YOURSHELL_IOS_RUNTIME_SHA256" ]]; then
        echo "error: runtime checksum mismatch." >&2
        echo "expected: $YOURSHELL_IOS_RUNTIME_SHA256" >&2
        echo "actual:   $actual_sha" >&2
        exit 1
    fi
    mv "$tmp_archive" "$ARCHIVE"
    trap - EXIT
fi

echo "Installing YourShell iOS runtime..."
tar -xzf "$ARCHIVE" -C "$ROOT"
printf '%s\n' "$YOURSHELL_IOS_RUNTIME_VERSION" >"$VERSION_MARKER"

missing="$(missing_paths)"
if [[ -n "$missing" ]]; then
    echo "error: runtime archive is incomplete:" >&2
    printf '  %s\n' $missing >&2
    exit 1
fi

echo "YourShell iOS runtime is ready ($YOURSHELL_IOS_RUNTIME_VERSION)."

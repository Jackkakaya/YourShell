#!/usr/bin/env bash

set -euo pipefail

RESOURCES="${1:?usage: prune-ios-runtime.sh <app-resources> <iphoneos|iphonesimulator>}"
PLATFORM="${2:?usage: prune-ios-runtime.sh <app-resources> <iphoneos|iphonesimulator>}"
WHEELS="$RESOURCES/PythonResources/wheels"

case "$PLATFORM" in
    iphoneos)
        if [[ -d "$WHEELS" ]]; then
            before="$(du -sk "$WHEELS" | awk '{print $1}')"
            rm -rf "$WHEELS"
            echo "Removed ${before} KiB simulator/offline wheel cache from device app."
        fi
        ;;
    iphonesimulator)
        if find "$WHEELS" -maxdepth 1 -type f -name '*-iphoneos.whl' \
            -print -quit 2>/dev/null | grep -q .; then
            echo "error: device wheel found in simulator app: $WHEELS" >&2
            exit 1
        fi
        echo "Kept simulator wheel cache for pip integration tests."
        ;;
    *)
        echo "error: unsupported platform: $PLATFORM" >&2
        exit 2
        ;;
esac

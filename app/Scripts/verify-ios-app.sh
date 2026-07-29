#!/usr/bin/env bash

set -euo pipefail

APP="${1:?usage: verify-ios-app.sh <app> <iphoneos|iphonesimulator>}"
PLATFORM="${2:?usage: verify-ios-app.sh <app> <iphoneos|iphonesimulator>}"

case "$PLATFORM" in
    iphoneos)
        EXPECTED_PLATFORM="IOS"
        FORBIDDEN_SUFFIX="iphonesimulator"
        REQUIRED_SUFFIX="iphoneos"
        ;;
    iphonesimulator)
        EXPECTED_PLATFORM="IOSSIMULATOR"
        FORBIDDEN_SUFFIX="iphoneos"
        REQUIRED_SUFFIX="iphonesimulator"
        ;;
    *)
        echo "error: unsupported platform: $PLATFORM" >&2
        exit 2
        ;;
esac

failures=0
fail() {
    echo "error: $*" >&2
    failures=$((failures + 1))
}

while IFS= read -r wrong; do
    fail "wrong-platform Python extension filename: ${wrong#$APP/}"
done < <(find "$APP" -type f -name "*cpython-*-${FORBIDDEN_SUFFIX}.so" -print)

for module in select _socket _ssl math; do
    if ! find "$APP/PythonResources/python/lib/python3.14/lib-dynload" \
        -maxdepth 1 -name "${module}.cpython-314-${REQUIRED_SUFFIX}.so" \
        -print -quit | grep -q .; then
        fail "missing required Python module: $module ($REQUIRED_SUFFIX)"
    fi
done

if [[ ! -s "$APP/PythonResources/cacert.pem" ]]; then
    fail "missing or empty Python TLS CA bundle"
fi

while IFS= read -r binary; do
    build_info="$(xcrun vtool -show-build "$binary" 2>/dev/null || true)"
    if [[ -z "$build_info" ]]; then
        continue
    fi
    if ! grep -Eq "platform ${EXPECTED_PLATFORM}$" <<<"$build_info"; then
        fail "wrong Mach-O platform in ${binary#$APP/}"
    fi

    if [[ "$PLATFORM" == "iphoneos" ]] \
        && ! codesign --verify --strict "$binary" >/dev/null 2>&1; then
        fail "unsigned or invalid Mach-O: ${binary#$APP/}"
    fi

    while IFS= read -r dependency; do
        [[ "$dependency" == @rpath/* ]] || continue
        relative="${dependency#@rpath/}"
        if [[ -e "$APP/Frameworks/$relative" ]]; then
            continue
        fi
        basename_dependency="$(basename "$relative")"
        if ! find "$APP" -type f -name "$basename_dependency" -print -quit \
            | grep -q .; then
            fail "unresolved @rpath dependency $dependency from ${binary#$APP/}"
        fi
    done < <(otool -L "$binary" 2>/dev/null | tail -n +2 | sed 's/^[[:space:]]*//; s/ (.*$//')
done < <(
    find "$APP" -type f -print \
        | while IFS= read -r candidate; do
            if file -b "$candidate" | grep -q 'Mach-O'; then
                printf '%s\n' "$candidate"
            fi
        done
)

for framework in Python NodeMobile; do
    if [[ ! -d "$APP/Frameworks/${framework}.framework" ]]; then
        fail "missing embedded framework: ${framework}.framework"
    fi
done

if [[ "$PLATFORM" == "iphoneos" ]] \
    && ! codesign --verify --deep --strict "$APP" >/dev/null 2>&1; then
    fail "final app bundle signature is invalid"
fi

if ((failures != 0)); then
    echo "iOS app verification failed: $failures issue(s)" >&2
    exit 1
fi

echo "iOS app verification passed: $PLATFORM"

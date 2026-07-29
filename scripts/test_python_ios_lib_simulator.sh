#!/bin/bash
#
# Build YourShell, install it on a booted iOS simulator, and execute the
# python-ios-lib runtime suite through the app's ASHELL_EXEC integration path.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECT="$ROOT/app/AShellRS.xcodeproj"
SCHEME="AShellRS"
BUNDLE_ID="dev.sumin.ashellrs"
TEST_SOURCE="$ROOT/tests/python_ios_lib_runtime.py"
DERIVED_DATA="${YOURSHELL_TEST_DERIVED_DATA:-/tmp/yourshell-python-ios-lib-tests}"
TIMEOUT_SECONDS="${YOURSHELL_TEST_TIMEOUT_SECONDS:-180}"
UDID="${1:-${YOURSHELL_SIMULATOR_UDID:-}}"

if [[ -z "$UDID" ]]; then
    UDID="$(
        xcrun simctl list devices booted \
            | awk -F '[()]' '/Booted/ { print $2; exit }'
    )"
fi

if [[ -z "$UDID" ]]; then
    echo "python-ios-lib test: no booted iOS simulator found" >&2
    exit 2
fi

if [[ ! -f "$TEST_SOURCE" ]]; then
    echo "python-ios-lib test: missing $TEST_SOURCE" >&2
    exit 2
fi

echo "python-ios-lib test: building for simulator $UDID"
xcodegen generate \
    --spec "$ROOT/app/project.yml" \
    --project "$ROOT/app"
xcodebuild \
    -quiet \
    -project "$PROJECT" \
    -scheme "$SCHEME" \
    -configuration Debug \
    -destination "id=$UDID" \
    -derivedDataPath "$DERIVED_DATA" \
    -disablePackageRepositoryCache \
    -scmProvider system \
    build \
    CODE_SIGNING_ALLOWED=NO

APP="$DERIVED_DATA/Build/Products/Debug-iphonesimulator/AShellRS.app"
if [[ ! -d "$APP" ]]; then
    echo "python-ios-lib test: build succeeded but app is missing: $APP" >&2
    exit 2
fi

xcrun simctl terminate "$UDID" "$BUNDLE_ID" 2>/dev/null || true
xcrun simctl install "$UDID" "$APP"

DATA_CONTAINER="$(xcrun simctl get_app_container "$UDID" "$BUNDLE_ID" data)"
DOCUMENTS="$DATA_CONTAINER/Documents"
mkdir -p "$DOCUMENTS"
cp -f "$TEST_SOURCE" "$DOCUMENTS/python_ios_lib_runtime.py"

OUTPUT="$DOCUMENTS/exec_out.txt"
if [[ -e "$OUTPUT" ]]; then
    mv -f "$OUTPUT" "$DOCUMENTS/exec_out.previous.txt"
fi

echo "python-ios-lib test: running through ASHELL_EXEC"
xcrun simctl launch \
    "$UDID" \
    "$BUNDLE_ID" \
    "ASHELL_EXEC=python3 python_ios_lib_runtime.py" >/dev/null

deadline=$((SECONDS + TIMEOUT_SECONDS))
while ((SECONDS < deadline)); do
    if [[ -f "$OUTPUT" ]] \
        && grep -q '^PYIOSLIB_RESULT ' "$OUTPUT"; then
        break
    fi
    sleep 1
done

xcrun simctl terminate "$UDID" "$BUNDLE_ID" 2>/dev/null || true

if [[ ! -f "$OUTPUT" ]]; then
    echo "python-ios-lib test: timed out without exec_out.txt" >&2
    exit 1
fi

cat "$OUTPUT"

if ! grep -q '^PYIOSLIB_RESULT PASS ' "$OUTPUT"; then
    echo "python-ios-lib test: runtime suite failed" >&2
    exit 1
fi

echo "python-ios-lib test: PASS"

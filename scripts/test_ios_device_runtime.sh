#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEVICE="${1:-${YOURSHELL_DEVICE_UDID:-}}"
TEAM="${YOURSHELL_DEVELOPMENT_TEAM:-35PQA6NV57}"
DERIVED_DATA="${YOURSHELL_DEVICE_DERIVED_DATA:-/tmp/yourshell-device-runtime-tests}"
BUNDLE_ID="dev.sumin.ashellrs"
APP="$DERIVED_DATA/Build/Products/Debug-iphoneos/AShellRS.app"

if [[ -z "$DEVICE" ]]; then
    echo "usage: $0 <device-udid-or-name>" >&2
    exit 2
fi

xcodegen generate --spec "$ROOT/app/project.yml" --project "$ROOT/app"
xcodebuild -quiet \
    -project "$ROOT/app/AShellRS.xcodeproj" \
    -scheme AShellRS \
    -configuration Debug \
    -destination "platform=iOS,id=$DEVICE" \
    -derivedDataPath "$DERIVED_DATA" \
    -allowProvisioningUpdates \
    DEVELOPMENT_TEAM="$TEAM" \
    CODE_SIGN_STYLE=Automatic \
    CODE_SIGNING_ALLOWED=YES \
    build

codesign --verify --deep --strict "$APP"
"$ROOT/app/Scripts/verify-ios-app.sh" "$APP" iphoneos
xcrun devicectl device install app --device "$DEVICE" "$APP"

run_case() {
    local name="$1"
    local command="$2"
    local expected="$3"
    local attempts="${4:-30}"
    local marker="__YOURSHELL_CASE_${name}_DONE__"
    local wrapped="echo __YOURSHELL_CASE_${name}_BEGIN__; ${command}; ys_case_rc=\$?; echo ${marker} rc=\$ys_case_rc"
    local output_dir
    output_dir="$(mktemp -d "/tmp/yourshell-${name}.XXXXXX")"

    echo "RUN  $name"
    DEVICECTL_CHILD_ASHELL_EXEC="$wrapped" \
    xcrun devicectl device process launch \
        --device "$DEVICE" \
        --terminate-existing \
        "$BUNDLE_ID"

    local deadline=$((SECONDS + attempts))
    local attempt=0
    while ((SECONDS < deadline)); do
        attempt=$((attempt + 1))
        local output="$output_dir/exec_out-$attempt.txt"
        if xcrun devicectl device copy from \
            --quiet \
            --device "$DEVICE" \
            --domain-type appDataContainer \
            --domain-identifier "$BUNDLE_ID" \
            --source Documents/exec_out.txt \
            --destination "$output" 2>/dev/null \
            && grep -Fq "$marker" "$output"; then
            if grep -Fq "$expected" "$output" \
                && grep -Fq "$marker rc=0" "$output"; then
                echo "PASS $name"
                return 0
            fi
            echo "FAIL $name" >&2
            cat "$output" >&2
            return 1
        fi
        sleep 1
    done

    echo "FAIL $name: host watchdog expired after ${attempts}s" >&2
    local latest
    latest="$(find "$output_dir" -type f -name 'exec_out-*.txt' -print \
        | sort -V | tail -n 1)"
    [[ -n "$latest" ]] && cat "$latest" >&2
    return 1
}

run_case python-version "python3 --version" "Python 3.14"
run_case pip-version "pip --version" "pip 26"
run_case python-native-modules \
    "python3 -c 'import select, socket, ssl, math; print(\"native-modules-ok\")'" \
    "native-modules-ok"
run_case node-runtime \
    "node -e 'const fs=require(\"fs\"); console.log(process.platform, typeof fs.readFileSync)'" \
    "ios function"
run_case npm-install-require \
    "rm -rf npm-gate && mkdir npm-gate && cd npm-gate && npm init -y && npm install --verbose is-odd@3 && node -e 'console.log(\"npm-install-ok\", require(\"is-odd\")(7))'" \
    "npm-install-ok true" \
    180

xcrun devicectl device copy to \
    --quiet \
    --device "$DEVICE" \
    --domain-type appDataContainer \
    --domain-identifier "$BUNDLE_ID" \
    --source "$ROOT/tests/python_ios_lib_runtime.py" \
    --destination Documents/python_ios_lib_runtime.py
run_case python-ios-lib-runtime \
    "python3 python_ios_lib_runtime.py" \
    "PYIOSLIB_RESULT PASS tests=10" \
    240

# Network/runtime cases run one app process at a time. Embedded runtimes cannot
# always be interrupted from inside the process, so the Mac-side poll loop is
# the hard watchdog. Never redirect stderr here: a failure must identify
# whether DNS, TLS, package resolution, wheel selection, or import failed.
network_failures=0
run_network_case() {
    if ! run_case "$@"; then
        network_failures=$((network_failures + 1))
    fi
}

run_network_case curl-https \
    "curl --fail --show-error --location --connect-timeout 15 --max-time 30 -o curl-network.html https://example.com/ && wc -c curl-network.html" \
    "curl-network.html" \
    60
run_network_case wget-https \
    "wget -S -T 30 -O wget-network.html https://example.com/ && wc -c wget-network.html" \
    "wget-network.html" \
    60
run_network_case python-urllib-https \
    "python3 -c 'import socket, ssl, urllib.request; print(\"dns\", socket.gethostbyname(\"example.com\")); print(\"ca\", ssl.get_default_verify_paths()); print(\"urllib\", urllib.request.urlopen(\"https://example.com/\", timeout=30).status)'" \
    "urllib 200" \
    60
run_network_case pip-install-six \
    "pip install -vv --disable-pip-version-check --no-input --only-binary=:all: --upgrade six && python3 -c 'import six; print(\"pip-six\", six.__version__)'" \
    "pip-six" \
    120
run_network_case pip-install-rich \
    "pip install -vv --disable-pip-version-check --no-input --only-binary=:all: --upgrade rich && python3 -c 'from rich.console import Console; Console(force_terminal=False).print(\"pip-rich\", 42)'" \
    "pip-rich 42" \
    180
run_network_case pip-install-document-stack \
    "pip install -v --disable-pip-version-check --no-input --only-binary=:all: fpdf2 pypdf requests && python3 -c 'import fpdf, pypdf, requests, PIL; print(\"pip-docs\", fpdf.__version__, pypdf.__version__)'" \
    "pip-docs" \
    240
run_network_case requests-https \
    "python3 -c 'import requests; r=requests.get(\"https://example.com/\", timeout=30); print(\"requests\", r.status_code, len(r.content)>100)'" \
    "requests 200 True" \
    60
run_network_case pip-prebundled-data \
    "pip install -vv --disable-pip-version-check --no-input --only-binary=:all: numpy pandas && python3 -c 'import numpy as np, pandas as pd; print(\"pip-data\", int(pd.Series(np.array([1,2,3])).mean()))'" \
    "pip-data 2" \
    180
run_network_case pip-install-pptx \
    "pip install -v --disable-pip-version-check --no-input --only-binary=:all: --index-url https://pypi.org/simple python-pptx && python3 -c 'from pptx import Presentation; p=Presentation(); p.slides.add_slide(p.slide_layouts[0]); p.save(\"network-test.pptx\"); print(\"pip-pptx\", len(p.slides))'" \
    "pip-pptx 1" \
    240
run_network_case npm-install-cowsay \
    "rm -rf npm-cowsay-gate && mkdir npm-cowsay-gate && cd npm-cowsay-gate && npm init -y && npm install --verbose cowsay && node -e 'console.log(\"npm-cowsay\", require(\"cowsay\").say({text:\"ok\"}).includes(\"ok\"))'" \
    "npm-cowsay true" \
    240

if ((network_failures != 0)); then
    echo "FAIL network/runtime battery: $network_failures case(s)" >&2
    exit 1
fi

# Clear any previous incremental report, then run every registered shell,
# Python and Node scenario through the real iOS host.
run_case clear-selftest-report \
    "rm -f selftest_report.txt; echo selftest-report-cleared" \
    "selftest-report-cleared"

DEVICECTL_CHILD_ASHELL_SELFTEST=1 \
DEVICECTL_CHILD_YS_SELFTEST_SKIP_NETWORK=1 \
xcrun devicectl device process launch \
    --device "$DEVICE" \
    --terminate-existing \
    "$BUNDLE_ID"

selftest_dir="$(mktemp -d /tmp/yourshell-selftest.XXXXXX)"
for _ in $(seq 1 900); do
    if xcrun devicectl device copy from \
        --quiet \
        --device "$DEVICE" \
        --domain-type appDataContainer \
        --domain-identifier "$BUNDLE_ID" \
        --source Documents/selftest_report.txt \
        --destination "$selftest_dir/selftest_report.txt" 2>/dev/null; then
        if grep -Eq '^=== [0-9]+/[0-9]+ passed ===$' \
            "$selftest_dir/selftest_report.txt"; then
            if grep -q '^FAIL ' "$selftest_dir/selftest_report.txt"; then
                cat "$selftest_dir/selftest_report.txt" >&2
                exit 1
            fi
            summary="$(tail -n 1 "$selftest_dir/selftest_report.txt")"
            passed="${summary#=== }"
            passed="${passed%%/*}"
            total="${summary#*/}"
            total="${total%% passed*}"
            if [[ "$passed" != "$total" ]]; then
                cat "$selftest_dir/selftest_report.txt" >&2
                exit 1
            fi
            tail -n 5 "$selftest_dir/selftest_report.txt"
            echo "PASS full-selftest-battery"
            echo "YourShell device runtime tests passed"
            exit 0
        fi
    fi
    sleep 1
done

echo "FAIL full-selftest-battery: timed out" >&2
[[ -f "$selftest_dir/selftest_report.txt" ]] \
    && cat "$selftest_dir/selftest_report.txt" >&2
exit 1

#!/usr/bin/env bash
# Cross-compile Python native wheel(s) for iOS — CPython 3.14, via the BeeWare
# Python-Apple-support cross-venv toolchain shipped inside Python.xcframework.
#
# Usage:
#   tools/build-ios-wheel.sh sim|device <pkg-spec> [more specs...]
# e.g.
#   tools/build-ios-wheel.sh sim    "lxml==6.1.1"
#   tools/build-ios-wheel.sh device "lxml==6.1.1"
#
# Output wheels land in YourShell/ios-wheels/. lxml needs a one-line patch to its
# buildlibxml.py (inject --host so the bundled libxml2/libxslt/iconv autotools
# builds skip un-runnable cross test programs); this script applies it.
set -euo pipefail

XCF="$HOME/codejk/YourShell/vendor/python-ios/Python.xcframework"
PY=/opt/homebrew/bin/python3.14
OUT="$HOME/codejk/YourShell/ios-wheels"
WORK="$(mktemp -d)"

case "${1:-}" in
  sim)
    SLICE="$XCF/ios-arm64_x86_64-simulator"
    CONFIG="$SLICE/platform-config/arm64-iphonesimulator"
    PREFIX=arm64-apple-ios-simulator ;;
  device)
    SLICE="$XCF/ios-arm64"
    CONFIG="$SLICE/platform-config/arm64-iphoneos"
    PREFIX=arm64-apple-ios ;;
  *) echo "usage: $0 sim|device <pkg-spec>..."; exit 1 ;;
esac
shift

cd "$WORK"
"$PY" -m venv cross-venv
"$PY" "$CONFIG/make_cross_venv.py" "$WORK/cross-venv" "$CONFIG"
# shellcheck disable=SC1091
source cross-venv/bin/activate
export PATH="$SLICE/bin:$PATH"
pip install -q -U setuptools wheel

# BeeWare CC wrappers embed -target/-isysroot; override the macOS -arch defaults
# lxml's buildlibxml injects, and point the autotools builds at the iOS toolchain.
export IPHONEOS_DEPLOYMENT_TARGET=13.0
export CC="$PREFIX-clang" CXX="$PREFIX-clang++" AR="$PREFIX-ar"
export CFLAGS="-O3" LDFLAGS="" CPPFLAGS=""
export CROSS_HOST=aarch64-apple-darwin   # triggers --host in the patched lxml build

mkdir -p "$OUT"
for spec in "$@"; do
  if [[ "$spec" == lxml* ]]; then
    pip download --no-deps --no-binary :all: "$spec" -d src
    tar xzf src/lxml-*.tar.gz -C src
    d="$(ls -d "$WORK"/src/lxml-*/ | head -1)"
    "$PY" - "$d/buildlibxml.py" <<'PYEOF'
import sys
path = sys.argv[1]
src = open(path).read()
anchor = "                     '--prefix=%s' % prefix,\n                     ]"
patch = anchor + ("\n    _cross_host = os.environ.get('CROSS_HOST')"
                  "\n    if _cross_host:"
                  "\n        configure_cmd.append('--host=%s' % _cross_host)")
if "CROSS_HOST" not in src:
    open(path, "w").write(src.replace(anchor, patch, 1))
    print("patched buildlibxml.py with --host injection")
PYEOF
    STATIC_DEPS=true pip wheel "$d" --no-build-isolation -w "$OUT" -v
  else
    pip wheel "$spec" --no-build-isolation -w "$OUT"
  fi
done
echo "== wheels in $OUT =="
ls -la "$OUT"

#!/bin/sh
# Assembles the bundled Python home for the app from the vendored
# Python-Apple-support package (simulator slice). Output: app/PythonResources/python
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
XC="$ROOT/vendor/python-ios/Python.xcframework"
SLICE="$XC/ios-arm64_x86_64-simulator"
OUT="$ROOT/app/PythonResources/python"

rm -rf "$OUT"
mkdir -p "$OUT/lib"
# Shared pure-python stdlib
cp -R "$XC/lib/python3.14" "$OUT/lib/python3.14"
# Strip the test suite (approx half the size, never needed at runtime)
rm -rf "$OUT/lib/python3.14/test" "$OUT/lib/python3.14/idlelib" "$OUT/lib/python3.14/turtledemo"
# Simulator slice: sysconfig + native extension modules
cp -R "$SLICE/lib-arm64/python3.14/"* "$OUT/lib/python3.14/"
du -sh "$OUT"

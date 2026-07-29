#!/bin/bash
#
# Finalize python-ios-lib resources after SwiftPM copies them into the app.
# The upstream packages contain native CPython extensions, so they must be
# individually signed.  The current upstream build is device-arm64 only; for
# an arm64 simulator build the ABI is compatible, but Mach-O's platform tag
# and CPython extension suffix must be changed before dyld/importlib accepts
# the files.

set -euo pipefail

APP="${TARGET_BUILD_DIR}/${WRAPPER_NAME}"
FRAMEWORKS="${TARGET_BUILD_DIR}/${FRAMEWORKS_FOLDER_PATH}"
IDENTITY="${EXPANDED_CODE_SIGN_IDENTITY:--}"
PYTHON_XCFRAMEWORK="$PROJECT_DIR/../vendor/python-ios/Python.xcframework"
PYTHON_STDLIB="$APP/PythonResources/python/lib/python3.14"

[[ -d "$APP" ]] || exit 0

mkdir -p "$FRAMEWORKS"

# PythonResources is a platform-neutral folder reference in Xcode, but
# lib-dynload and sysconfig files are platform-specific Mach-O artifacts.
# Replace them on every build from the matching XCFramework slice. Never let
# a simulator extension silently ship in a device app (or vice versa).
case "${PLATFORM_NAME:-}" in
    iphoneos)
        PYTHON_PLATFORM_STDLIB="$PYTHON_XCFRAMEWORK/ios-arm64/lib-arm64/python3.14"
        PYTHON_EXTENSION_SUFFIX="iphoneos"
        ;;
    iphonesimulator)
        PYTHON_PLATFORM_STDLIB="$PYTHON_XCFRAMEWORK/ios-arm64_x86_64-simulator/lib-arm64/python3.14"
        PYTHON_EXTENSION_SUFFIX="iphonesimulator"
        ;;
    *)
        echo "error: unsupported Python runtime platform: ${PLATFORM_NAME:-unset}" >&2
        exit 1
        ;;
esac

if [[ ! -d "$PYTHON_PLATFORM_STDLIB/lib-dynload" ]]; then
    echo "error: missing Python platform stdlib: $PYTHON_PLATFORM_STDLIB" >&2
    exit 1
fi
if [[ ! -d "$PYTHON_STDLIB" ]]; then
    echo "error: PythonResources stdlib was not copied into the app: $PYTHON_STDLIB" >&2
    exit 1
fi

rm -rf "$PYTHON_STDLIB/lib-dynload"
cp -R "$PYTHON_PLATFORM_STDLIB/lib-dynload" "$PYTHON_STDLIB/"
rm -f "$PYTHON_STDLIB"/_sysconfigdata__*.py \
      "$PYTHON_STDLIB"/_sysconfig_vars__*.json \
      "$PYTHON_STDLIB/build-details.json"
cp "$PYTHON_PLATFORM_STDLIB"/_sysconfigdata__*.py "$PYTHON_STDLIB/" 2>/dev/null || true
cp "$PYTHON_PLATFORM_STDLIB"/_sysconfig_vars__*.json "$PYTHON_STDLIB/" 2>/dev/null || true
cp "$PYTHON_PLATFORM_STDLIB/build-details.json" "$PYTHON_STDLIB/" 2>/dev/null || true

for module in select _socket _ssl math; do
    if ! find "$PYTHON_STDLIB/lib-dynload" \
        -maxdepth 1 -name "${module}.cpython-314-${PYTHON_EXTENSION_SUFFIX}.so" \
        -print -quit | grep -q .; then
        echo "error: packaged Python runtime lacks ${module} for ${PYTHON_EXTENSION_SUFFIX}" >&2
        exit 1
    fi
done

# iOS has no /etc/ssl/cert.pem. Reuse pip's pinned certifi bundle as the
# process-wide Python trust store so stdlib urllib and third-party clients do
# not fail TLS verification while pip itself appears healthy.
PIP_WHEEL="$PYTHON_STDLIB/ensurepip/_bundled"/pip-*.whl
if ! /usr/bin/unzip -p $PIP_WHEEL \
    pip/_vendor/certifi/cacert.pem \
    > "$APP/PythonResources/cacert.pem"; then
    echo "error: could not extract Python CA bundle from $PIP_WHEEL" >&2
    exit 1
fi
if [[ ! -s "$APP/PythonResources/cacert.pem" ]]; then
    echo "error: packaged Python CA bundle is empty" >&2
    exit 1
fi

# SwiftPM package resource copies may be scheduled after this target phase.
# Always process their product bundles as well as the current app contents, so
# a late copy cannot restore an unpatched or unsigned file.
scan_roots=("$APP")
while IFS= read -r -d '' bundle; do
    scan_roots+=("$bundle")
done < <(
    find "$TARGET_BUILD_DIR" -maxdepth 1 -type d \
        \( -name 'python-ios-lib_*.bundle' -o -name 'python_ios_lib_*.bundle' \) \
        -print0
)

# The vendored Matplotlib product needs two hard native dependencies that
# upstream does not declare as target resources. Copy them from the pinned
# local package, so the build never depends on a SwiftPM checkout in
# DerivedData.
PYIOS_VENDOR="$PROJECT_DIR/../vendor/python-ios-lib"
MATPLOTLIB_EXTRAS="$APP/python-ios-lib_MatplotlibExtras.bundle"
if [[ -d "$PYIOS_VENDOR/MatplotlibExtras" ]]; then
    mkdir -p "$MATPLOTLIB_EXTRAS"
    for item in \
        kiwisolver \
        kiwisolver-1.5.0.dist-info \
        contourpy \
        contourpy-1.3.3.dist-info; do
        source_item="$PYIOS_VENDOR/MatplotlibExtras/$item"
        [[ -e "$source_item" ]] || continue
        cp -R -f "$source_item" "$MATPLOTLIB_EXTRAS/"
    done
fi

# The package's lightweight mplot3d compatibility class is registered by
# matplotlib.projections during every pyplot import. Projection classes must
# expose a `name`, but the upstream shim currently omits it, which prevents
# even the non-GUI Agg backend from loading.
for matplotlib_bundle in \
    "$APP"/python-ios-lib_Matplotlib.bundle \
    "$APP"/python_ios_lib_Matplotlib.bundle \
    "$TARGET_BUILD_DIR"/python-ios-lib_Matplotlib.bundle \
    "$TARGET_BUILD_DIR"/python_ios_lib_Matplotlib.bundle; do
    shim="$matplotlib_bundle/mpl_toolkits/mplot3d/__init__.py"
    [[ -f "$shim" ]] || continue
    if ! grep -q "^    name = '3d'$" "$shim"; then
        sed -i '' "/^class Axes3D:$/a\\
    name = '3d'
" "$shim"
    fi
done

# SciPy extension modules reference these by @rpath. SwiftPM ships them as
# resources inside SciPy.bundle, which is not an app rpath, so also place a
# copy in Frameworks.
for scipy_bundle in \
    "$APP"/python-ios-lib_SciPy.bundle \
    "$APP"/python_ios_lib_SciPy.bundle; do
    [[ -d "$scipy_bundle/scipy_runtime" ]] || continue
    while IFS= read -r -d '' runtime; do
        cp -f "$runtime" "$FRAMEWORKS/$(basename "$runtime")"
    done < <(find "$scipy_bundle/scipy_runtime" -type f -name '*.dylib' -print0)
done

# libscipy_blas_stubs is required by scipy.linalg but is not currently copied
# into the SciPy resource bundle. It is present in the package's auxiliary
# framework directory, alongside the canonical versions of the two runtime
# libraries above.
for runtime_dir in \
    "$PYIOS_VENDOR/Frameworks/scipy_aux"; do
    [[ -d "$runtime_dir" ]] || continue
    while IFS= read -r -d '' runtime; do
        cp -f "$runtime" "$FRAMEWORKS/$(basename "$runtime")"
    done < <(find "$runtime_dir" -type f -name '*.dylib' -print0)
done

replatform_for_simulator() {
    local binary="$1"
    local patched="${binary}.iossim"

    if xcrun vtool -show-build "$binary" 2>/dev/null \
        | grep -Eq 'platform (IOS|ios)$'; then
        xcrun vtool \
            -set-build-version iossim \
            "${IPHONEOS_DEPLOYMENT_TARGET:-17.0}" \
            "${SDK_VERSION:-17.0}" \
            -replace -output "$patched" "$binary"
        mv -f "$patched" "$binary"
    fi
}

sign_native() {
    codesign --force --sign "$IDENTITY" --timestamp=none \
        --preserve-metadata=identifier,entitlements,flags "$1" \
        2>/dev/null \
        || codesign --force --sign "$IDENTITY" --timestamp=none "$1"
}

if [[ "${PLATFORM_NAME:-}" == "iphonesimulator" ]]; then
    for scan_root in "${scan_roots[@]}"; do
        while IFS= read -r -d '' binary; do
            replatform_for_simulator "$binary"
        done < <(
            find "$scan_root" -type f \( -name '*.so' -o -name '*.dylib' \) -print0
        )

        # CPython's simulator import machinery looks for
        # .cpython-314-iphonesimulator.so, not the upstream iphoneos spelling.
        while IFS= read -r -d '' binary; do
            mv -f "$binary" \
                "${binary%.cpython-314-iphoneos.so}.cpython-314-iphonesimulator.so"
        done < <(
            find "$scan_root" -type f -name '*.cpython-314-iphoneos.so' -print0
        )
    done
fi

for scan_root in "${scan_roots[@]}"; do
    while IFS= read -r -d '' binary; do
        sign_native "$binary"
    done < <(
        find "$scan_root" -type f \( -name '*.so' -o -name '*.dylib' \) -print0
    )
done

# Xcode normally signs embedded frameworks because project.yml marks them
# codeSign: true. Keep this explicit verification as a packaging contract:
# an unsigned runtime framework must fail the build, not fail later at install.
if [[ "${PLATFORM_NAME:-}" == "iphoneos" ]]; then
    for framework in \
        "$FRAMEWORKS/Python.framework" \
        "$FRAMEWORKS/NodeMobile.framework"; do
        if [[ ! -d "$framework" ]]; then
            echo "error: missing embedded framework: $framework" >&2
            exit 1
        fi
        sign_native "$framework"
        if ! codesign --verify --strict "$framework"; then
            echo "error: invalid embedded framework signature: $framework" >&2
            exit 1
        fi
    done

fi

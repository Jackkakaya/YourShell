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

[[ -d "$APP" ]] || exit 0

mkdir -p "$FRAMEWORKS"

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
    "$APP"/python_ios_lib_Matplotlib.bundle; do
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
    while IFS= read -r -d '' binary; do
        replatform_for_simulator "$binary"
    done < <(
        find "$APP" -type f \( -name '*.so' -o -name '*.dylib' \) -print0
    )

    # CPython's simulator import machinery looks for
    # .cpython-314-iphonesimulator.so, not the upstream iphoneos spelling.
    while IFS= read -r -d '' binary; do
        mv -f "$binary" "${binary%.cpython-314-iphoneos.so}.cpython-314-iphonesimulator.so"
    done < <(find "$APP" -type f -name '*.cpython-314-iphoneos.so' -print0)
fi

while IFS= read -r -d '' binary; do
    sign_native "$binary"
done < <(
    find "$APP" -type f \( -name '*.so' -o -name '*.dylib' \) -print0
)

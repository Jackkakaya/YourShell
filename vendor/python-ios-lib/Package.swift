// swift-tools-version: 5.9
//
// YourShell's vendored, minimal subset of:
// https://github.com/yu314-coder/python-ios-lib
//
// Upstream revision: a8c166b210d38bf09864a1ea16689315ecf5fc54

import PackageDescription

let package = Package(
    name: "python-ios-lib",
    platforms: [.iOS(.v17)],
    products: [
        .library(name: "NumPy", targets: ["NumPy"]),
        .library(name: "SciPy", targets: ["SciPy", "NumPy"]),
        .library(name: "Pandas", targets: ["Pandas", "NumPy", "Dateutil"]),
        .library(name: "Pillow", targets: ["Pillow"]),
        .library(name: "Lxml", targets: ["Lxml"]),
        .library(
            name: "Matplotlib",
            targets: ["Matplotlib", "Plotly", "Dateutil", "FontTools"]
        ),
        .library(name: "Pyparsing", targets: ["Pyparsing"]),
    ],
    targets: [
        .target(
            name: "NumPy",
            path: "Sources/NumPy",
            resources: [
                .copy("numpy"),
                .copy("numpy-2.3.5.post1.dist-info"),
            ]
        ),
        .target(
            name: "SciPy",
            dependencies: ["NumPy"],
            path: "Sources/SciPy",
            resources: [
                .copy("scipy"),
                .copy("scipy_runtime"),
                .copy("scipy-1.15.0.dist-info"),
            ]
        ),
        .target(
            name: "Pandas",
            dependencies: ["NumPy", "Dateutil"],
            path: "Sources/Pandas",
            resources: [
                .copy("pandas"),
                .copy("pytz"),
                .copy("pandas-2.2.3.dist-info"),
            ]
        ),
        .target(
            name: "Pillow",
            path: "Sources/Pillow",
            resources: [
                .copy("PIL"),
                .copy("pillow-11.0.0.dist-info"),
            ]
        ),
        .target(
            name: "Lxml",
            path: "Sources/Lxml",
            resources: [
                .copy("lxml"),
                .copy("lxml-6.1.1.dist-info"),
                .copy("flet_libxml2-2.15.3.dist-info"),
                .copy("flet_libxslt-1.1.45.dist-info"),
            ]
        ),
        .target(
            name: "Plotly",
            path: "Sources/Plotly",
            resources: [
                .copy("plotly"),
                .copy("_plotly_utils"),
                .copy("plotly-6.6.0.dist-info"),
            ]
        ),
        .target(
            name: "Dateutil",
            path: "Sources/Dateutil",
            resources: [
                .copy("dateutil"),
                .copy("six.pyc"),
            ]
        ),
        .target(
            name: "Matplotlib",
            dependencies: ["Plotly", "Dateutil", "FontTools"],
            path: "Sources/Matplotlib",
            resources: [
                .copy("matplotlib"),
                .copy("matplotlib-3.9.0.dist-info"),
                .copy("narwhals-1.16.0.dist-info"),
                .copy("packaging-26.0.dist-info"),
                .copy("narwhals"),
                .copy("packaging"),
                .copy("cycler"),
                .copy("mpl_toolkits"),
            ]
        ),
        .target(
            name: "FontTools",
            path: "Sources/FontTools",
            resources: [.copy("fontTools")]
        ),
        .target(
            name: "Pyparsing",
            path: "Sources/Pyparsing",
            resources: [
                .copy("pyparsing"),
                .copy("pyparsing-3.3.2.dist-info"),
            ]
        ),
    ]
)

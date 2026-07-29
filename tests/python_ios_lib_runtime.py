#!/usr/bin/env python3
"""Runtime integration tests for python-ios-lib inside YourShell on iOS.

This file is copied to the simulator's Documents directory and executed by
the app through ASHELL_EXEC. It intentionally exercises native code paths,
not only package metadata.
"""

from __future__ import annotations

import importlib
import importlib.metadata
import io
import math
import sys
import unittest


def package_version(distribution: str) -> str:
    version = importlib.metadata.version(distribution)
    if not version:
        raise AssertionError(f"{distribution}: empty package version")
    return version


class PythonIOSLibRuntimeTests(unittest.TestCase):
    def test_01_imports_and_versions(self) -> None:
        expected = {
            "numpy": "numpy",
            "scipy": "scipy",
            "pandas": "pandas",
            "PIL": "Pillow",
            "matplotlib": "matplotlib",
            "pyparsing": "pyparsing",
        }
        versions: dict[str, dict[str, str]] = {}
        for module_name, distribution in expected.items():
            with self.subTest(library=module_name):
                module = importlib.import_module(module_name)
                distribution_version = package_version(distribution)
                module_version = str(getattr(module, "__version__", ""))
                self.assertTrue(
                    module_version,
                    f"{module_name}: module __version__ is unavailable",
                )
                versions[module_name] = {
                    "module": module_version,
                    "distribution": distribution_version,
                }
                self.assertIsNotNone(
                    getattr(module, "__file__", None),
                    f"{module_name}: module has no source path",
                )
        print("PYIOSLIB_VERSIONS", versions, flush=True)

    def test_02_numpy_ndarray_and_matrix(self) -> None:
        import numpy as np

        array = np.arange(6, dtype=np.float64).reshape(2, 3)
        product = array @ array.T
        np.testing.assert_allclose(
            product,
            np.array([[5.0, 14.0], [14.0, 50.0]]),
            err_msg="NumPy: ndarray matrix multiplication is incorrect",
        )
        matrix = np.array([[4.0, 7.0], [2.0, 6.0]])
        inverse = np.linalg.inv(matrix)
        np.testing.assert_allclose(
            matrix @ inverse,
            np.eye(2),
            atol=1e-12,
            err_msg="NumPy: native linalg inverse is incorrect",
        )

    def test_03_scipy_linalg_and_stats(self) -> None:
        import numpy as np
        from scipy import linalg, stats

        coefficients = np.array([[3.0, 1.0], [1.0, 2.0]])
        result = linalg.solve(coefficients, np.array([9.0, 8.0]))
        np.testing.assert_allclose(
            result,
            np.array([2.0, 3.0]),
            atol=1e-12,
            err_msg="SciPy: native linalg.solve returned the wrong result",
        )
        self.assertAlmostEqual(
            float(stats.norm.cdf(0.0)),
            0.5,
            places=12,
            msg="SciPy: stats.norm.cdf native path returned the wrong result",
        )

    def test_04_pandas_groupby_and_csv_roundtrip(self) -> None:
        import pandas as pd

        frame = pd.DataFrame(
            {
                "team": ["red", "blue", "red", "blue"],
                "score": [3, 5, 7, 11],
            }
        )
        grouped = frame.groupby("team", sort=True)["score"].sum()
        self.assertEqual(
            grouped.to_dict(),
            {"blue": 16, "red": 10},
            "Pandas: DataFrame groupby result is incorrect",
        )

        csv_buffer = io.StringIO()
        frame.to_csv(csv_buffer, index=False)
        restored = pd.read_csv(io.StringIO(csv_buffer.getvalue()))
        pd.testing.assert_frame_equal(
            frame,
            restored,
            obj="Pandas: CSV roundtrip",
        )

    def test_05_pillow_png_roundtrip(self) -> None:
        from PIL import Image

        image = Image.new("RGBA", (8, 6), (12, 34, 56, 255))
        png = io.BytesIO()
        image.save(png, format="PNG")
        self.assertTrue(
            png.getvalue().startswith(b"\x89PNG\r\n\x1a\n"),
            "Pillow: output does not have a PNG signature",
        )
        png.seek(0)
        with Image.open(png) as restored:
            restored.load()
            self.assertEqual(restored.format, "PNG", "Pillow: format changed")
            self.assertEqual(restored.size, (8, 6), "Pillow: size changed")
            self.assertEqual(
                restored.getpixel((0, 0)),
                (12, 34, 56, 255),
                "Pillow: pixel data changed",
            )

    def test_06_matplotlib_agg_png(self) -> None:
        import matplotlib

        matplotlib.use("Agg", force=True)
        from matplotlib import pyplot as plt

        figure, axes = plt.subplots(figsize=(2.0, 1.5), dpi=80)
        axes.plot([0, 1, 2], [0, 1, 4])
        png = io.BytesIO()
        figure.savefig(png, format="png")
        plt.close(figure)
        self.assertGreater(
            len(png.getvalue()),
            500,
            "Matplotlib: Agg backend produced an unexpectedly small PNG",
        )
        self.assertTrue(
            png.getvalue().startswith(b"\x89PNG\r\n\x1a\n"),
            "Matplotlib: Agg backend output is not PNG",
        )

    def test_07_pandas_to_matplotlib(self) -> None:
        import matplotlib

        matplotlib.use("Agg", force=True)
        from matplotlib import pyplot as plt
        import pandas as pd

        frame = pd.DataFrame({"value": [2, 5, 3]}, index=["a", "b", "c"])
        axes = frame.plot(kind="bar", legend=False)
        png = io.BytesIO()
        axes.figure.savefig(png, format="png")
        plt.close(axes.figure)
        self.assertTrue(
            png.getvalue().startswith(b"\x89PNG\r\n\x1a\n"),
            "Pandas→Matplotlib: plot did not produce PNG output",
        )

    def test_08_numpy_to_scipy(self) -> None:
        import numpy as np
        from scipy import linalg

        values = np.array([[3.0, 1.0], [1.0, 3.0]])
        singular_values = linalg.svdvals(values)
        np.testing.assert_allclose(
            singular_values,
            np.array([4.0, 2.0]),
            atol=1e-12,
            err_msg="NumPy→SciPy: native SVD returned the wrong values",
        )
        self.assertTrue(
            math.isfinite(float(singular_values.sum())),
            "NumPy→SciPy: SVD produced a non-finite value",
        )

    def test_09_matplotlib_native_dependencies(self) -> None:
        import contourpy
        import kiwisolver
        import pyparsing

        for module, distribution in [
            (contourpy, "contourpy"),
            (kiwisolver, "kiwisolver"),
            (pyparsing, "pyparsing"),
        ]:
            with self.subTest(library=distribution):
                self.assertTrue(
                    package_version(distribution),
                    f"{distribution}: version metadata is unavailable",
                )
                self.assertIsNotNone(
                    module.__file__,
                    f"{distribution}: module has no source path",
                )

    def test_10_lxml_signed_native_extension(self) -> None:
        from lxml import etree

        root = etree.fromstring(b"<root><value>42</value></root>")
        self.assertEqual(root.findtext("value"), "42")


def main() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(
        PythonIOSLibRuntimeTests
    )
    result = unittest.TextTestRunner(
        stream=sys.stdout,
        verbosity=2,
    ).run(suite)
    if result.wasSuccessful():
        print(
            f"PYIOSLIB_RESULT PASS tests={result.testsRun}",
            flush=True,
        )
        return 0
    print(
        "PYIOSLIB_RESULT FAIL "
        f"tests={result.testsRun} failures={len(result.failures)} "
        f"errors={len(result.errors)}",
        flush=True,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())

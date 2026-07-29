import re
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def tracked_files() -> list[str]:
    result = subprocess.run(
        ["git", "ls-files"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return [
        path
        for path in result.stdout.splitlines()
        if (ROOT / path).exists()
    ]


class RepositoryContractTests(unittest.TestCase):
    def test_required_maintainer_documents_exist(self) -> None:
        for name in (
            "README.md",
            "CONTRIBUTING.md",
            "SECURITY.md",
            "CODE_OF_CONDUCT.md",
            "docs/README.md",
            "docs/TESTING.md",
        ):
            self.assertTrue((ROOT / name).is_file(), name)

    def test_readme_relative_links_resolve(self) -> None:
        text = (ROOT / "README.md").read_text(encoding="utf-8")
        links = re.findall(r"\[[^\]]+\]\(([^)]+)\)", text)
        relative = [
            target.split("#", 1)[0]
            for target in links
            if target
            and not target.startswith(("http://", "https://", "mailto:", "#"))
        ]
        missing = [target for target in relative if not (ROOT / target).exists()]
        self.assertEqual(missing, [])

    def test_root_has_no_accidental_npm_install(self) -> None:
        tracked = tracked_files()
        forbidden = [
            path
            for path in tracked
            if path == "package.json"
            or path == "package-lock.json"
            or path.startswith("node_modules/")
        ]
        self.assertEqual(forbidden, [])

    def test_research_documents_are_not_scattered_at_root(self) -> None:
        allowed = {
            "README.md",
            "CONTRIBUTING.md",
            "SECURITY.md",
            "CODE_OF_CONDUCT.md",
        }
        root_markdown = {
            path.name
            for path in ROOT.glob("*.md")
            if path.is_file()
        }
        self.assertEqual(root_markdown, allowed)

    def test_large_node_binaries_are_declared_as_lfs(self) -> None:
        attributes = (ROOT / ".gitattributes").read_text(encoding="utf-8")
        required = (
            "vendor/nodejs-mobile/NodeMobile.xcframework/"
            "ios-arm64/NodeMobile.framework/NodeMobile",
            "vendor/nodejs-mobile/NodeMobile.xcframework/"
            "ios-arm64_x86_64-simulator/NodeMobile.framework/NodeMobile",
        )
        for path in required:
            self.assertIn(f"{path} filter=lfs", attributes)

    def test_regular_git_blobs_fit_github_limit(self) -> None:
        result = subprocess.run(
            ["git", "ls-tree", "-r", "--long", "HEAD"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        oversized = []
        for line in result.stdout.splitlines():
            metadata, path = line.split("\t", 1)
            size = metadata.rsplit(" ", 1)[-1]
            if size != "-" and int(size) > 100 * 1024 * 1024:
                oversized.append(path)
        self.assertEqual(oversized, [])


if __name__ == "__main__":
    unittest.main()

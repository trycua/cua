"""Regression tests for public Python example import boundaries."""

from pathlib import Path
import re
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
PUBLIC_PYTHON_ROOTS = (REPO_ROOT / "samples",)
NATIVE_FLEET_EXAMPLES = (
    REPO_ROOT / "libs/python/cua-fleet/README.md",
    REPO_ROOT / "libs/fleet/sdk-bindings/examples/python/live_app_controlled.py",
)
FORBIDDEN_IMPORTS = (
    re.compile(r"^\s*(?:from|import)\s+cua_fleet\b"),
    re.compile(r"^\s*(?:from|import)\s+cyclops_sdk\b"),
    re.compile(r"^\s*(?:from|import)\s+cua_sandbox\.(?:runtime|transport)\b"),
)


def markdown_python_lines(path: Path):
    in_python_fence = False
    for line_number, line in enumerate(path.read_text().splitlines(), start=1):
        fence = re.match(r"^\s*```\s*([\w+-]*)", line)
        if fence:
            if in_python_fence:
                in_python_fence = False
            else:
                in_python_fence = fence.group(1).lower() in {"python", "py", "pycon"}
            continue
        if in_python_fence:
            yield line_number, line


class TestDocumentedPythonImports(unittest.TestCase):
    def test_public_examples_use_supported_sdk_imports(self) -> None:
        failures: list[str] = []
        markdown_files = [REPO_ROOT / "README.md"]
        markdown_files.extend((REPO_ROOT / "docs/content").rglob("*.mdx"))
        markdown_files.extend((REPO_ROOT / "libs").rglob("README.md"))

        for path in markdown_files:
            for line_number, line in markdown_python_lines(path):
                if any(pattern.match(line) for pattern in FORBIDDEN_IMPORTS):
                    failures.append(f"{path.relative_to(REPO_ROOT)}:{line_number}: {line.strip()}")

        python_files = []
        for root in PUBLIC_PYTHON_ROOTS:
            python_files.extend(root.rglob("*.py"))
        for examples_dir in REPO_ROOT.rglob("examples"):
            if examples_dir.is_dir():
                python_files.extend(examples_dir.rglob("*.py"))

        for path in python_files:
            for line_number, line in enumerate(path.read_text().splitlines(), start=1):
                if any(pattern.match(line) for pattern in FORBIDDEN_IMPORTS):
                    failures.append(f"{path.relative_to(REPO_ROOT)}:{line_number}: {line.strip()}")

        self.assertEqual([], failures, "Unsupported public example imports:\n" + "\n".join(failures))

        for path in NATIVE_FLEET_EXAMPLES:
            self.assertRegex(path.read_text(), r"(?m)^from fleet_sdk import ")


if __name__ == "__main__":
    unittest.main()

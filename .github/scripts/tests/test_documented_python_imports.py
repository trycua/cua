"""Regression tests for public Python example import boundaries."""

from pathlib import Path
import re
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]
CUSTOMER_SANDBOX_EXAMPLES = (REPO_ROOT / "libs/python/cua-fleet/README.md",)
FORBIDDEN_IMPORTS = (
    re.compile(r"^\s*(?:from|import)\s+cua_fleet\b"),
    re.compile(r"^\s*(?:from|import)\s+cyclops_sdk\b"),
    re.compile(r"^\s*(?:from|import)\s+fleet_sdk\b"),
    re.compile(r"^\s*(?:from|import)\s+cua_sandbox\.(?:runtime|transport)\b"),
)
LOW_LEVEL_NATIVE_TYPES = re.compile(
    r"\b(?:CyclopsClient|CyclopsConfiguration|CyclopsCredentials|"
    r"CyclopsTokenProviderConfiguration|HttpClient|HttpHeader|HttpRequest|HttpResponse|"
    r"AccessTokenProvider)\b"
)


def customer_example_lines():
    markdown_files = [REPO_ROOT / "README.md"]
    markdown_files.extend((REPO_ROOT / "docs/content").rglob("*.mdx"))
    markdown_files.extend((REPO_ROOT / "libs/python").rglob("README.md"))

    for path in markdown_files:
        in_python_fence = False
        for line_number, line in enumerate(path.read_text().splitlines(), start=1):
            fence = re.match(r"^\s*```\s*([\w+-]*)", line)
            if fence:
                if in_python_fence:
                    in_python_fence = False
                else:
                    in_python_fence = fence.group(1).lower() in {"python", "py", "pycon"}
                continue
            yield path, line_number, line, in_python_fence

    for path in (REPO_ROOT / "samples").rglob("*.py"):
        for line_number, line in enumerate(path.read_text().splitlines(), start=1):
            yield path, line_number, line, True


class TestDocumentedPythonImports(unittest.TestCase):
    def test_public_examples_exclude_unsupported_sdk_imports_and_types(self) -> None:
        failures: list[str] = []

        for path, line_number, line, is_python in customer_example_lines():
            reasons = []
            if is_python and any(pattern.match(line) for pattern in FORBIDDEN_IMPORTS):
                reasons.append("unsupported SDK import")
            if LOW_LEVEL_NATIVE_TYPES.search(line):
                reasons.append("low-level native client type")
            if reasons:
                failures.append(
                    f"{path.relative_to(REPO_ROOT)}:{line_number}: {', '.join(reasons)}: "
                    f"{line.strip()}"
                )

        self.assertEqual([], failures, "Unsupported public example API:\n" + "\n".join(failures))

    def test_customer_workflows_use_the_sandbox_sdk(self) -> None:
        for path in CUSTOMER_SANDBOX_EXAMPLES:
            self.assertRegex(path.read_text(), r"(?m)^from cua_sandbox import (?:Image, Sandbox|Sandbox)")


if __name__ == "__main__":
    unittest.main()

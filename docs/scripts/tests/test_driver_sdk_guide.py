"""Assert the Driver "Use the SDK" guide names the real ToolResult fields."""

import ast
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[3]
PAGE = ROOT / "docs/content/docs/cua-driver/guides/use-the-sdk.mdx"
PYTHON = ROOT / "libs/cua-driver/python/src/cua_driver/_native.py"
TYPESCRIPT = ROOT / "libs/cua-driver/typescript/src/native/cua_driver_sdk.ts"


def _guide_fields() -> list[str]:
    page = " ".join(PAGE.read_text().split())
    match = re.search(r"Results are `ToolResult` \(([^)]*)\)", page)
    assert match, "guide no longer lists the ToolResult fields"
    return re.findall(r"`([A-Za-z_]+)`", match.group(1))


def _python_fields() -> list[str]:
    module = ast.parse(PYTHON.read_text())
    for node in module.body:
        if isinstance(node, ast.ClassDef) and node.name == "ToolResult":
            for item in node.body:
                if isinstance(item, ast.FunctionDef) and item.name == "__init__":
                    return [argument.arg for argument in item.args.kwonlyargs]
    raise AssertionError("ToolResult.__init__ not found in the Python bindings")


def _typescript_fields() -> list[str]:
    match = re.search(
        r"export type ToolResult = \{(.*?)\n\}", TYPESCRIPT.read_text(), re.S
    )
    assert match, "ToolResult type not found in the TypeScript bindings"
    return re.findall(r"^\s*(\w+)\??:", match.group(1), re.M)


def _camel(name: str) -> str:
    head, *rest = name.split("_")
    return head + "".join(part.capitalize() for part in rest)


class DriverSdkGuideTests(unittest.TestCase):
    def test_tool_result_fields_match_the_python_bindings(self):
        self.assertEqual(_guide_fields(), _python_fields())

    def test_tool_result_fields_match_the_typescript_bindings(self):
        self.assertEqual(
            [_camel(name) for name in _guide_fields()], _typescript_fields()
        )


if __name__ == "__main__":
    unittest.main()

"""Python docs blocks build Cua Driver inputs the generated bindings accept (stdlib only).

    python3 -m unittest discover -s tests/e2e/cua-sdk/docs -p 'test_*.py'

The generated Python records take keyword-only arguments, and a field without
a UniFFI default is required even when its type is Optional. Driver examples
are `skip="no-runner"` (they need a desktop), so nothing runs them; this check
reads each block's `from cua_driver import ...` names and compares every
constructor call against `__init__` in `_native_contract.py`, without loading
the native library.
"""

from __future__ import annotations

import ast
import re
import unittest
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
CONTRACT = REPO / "libs/cua-driver/python/src/cua_driver/_native_contract.py"
SOURCES = (
    (REPO / "docs/content/docs", "*.mdx"),
    (REPO / "libs/cua-driver", "*.md"),
)
FENCE = re.compile(r"^[ \t]*```python[^\n]*\n(.*?)^[ \t]*```", re.S | re.M)


@dataclass(frozen=True)
class Signature:
    positional: tuple[str, ...]
    required_positional: int
    keywords: frozenset[str]
    required_keywords: frozenset[str]


def contract_signatures(source: str) -> dict[str, Signature]:
    """Map each generated class (nested ones dotted, e.g. ``ActionTarget.WINDOW``) to its ``__init__``."""
    out: dict[str, Signature] = {}

    def walk(body: list[ast.stmt], prefix: str) -> None:
        for node in body:
            if not isinstance(node, ast.ClassDef):
                continue
            name = prefix + node.name
            for item in node.body:
                if isinstance(item, ast.FunctionDef) and item.name == "__init__":
                    a = item.args
                    positional = tuple(x.arg for x in a.args[1:])
                    out[name] = Signature(
                        positional=positional,
                        required_positional=len(positional) - len(a.defaults),
                        keywords=frozenset(x.arg for x in a.kwonlyargs),
                        required_keywords=frozenset(
                            x.arg for x, d in zip(a.kwonlyargs, a.kw_defaults) if d is None
                        ),
                    )
            walk(node.body, name + ".")

    walk(ast.parse(source).body, "")
    return out


def call_errors(block: str, signatures: dict[str, Signature]) -> list[str]:
    """Constructor calls in ``block`` that the generated ``__init__`` would reject with TypeError."""
    tree = ast.parse(block)
    imported = {
        alias.asname or alias.name: alias.name
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.module == "cua_driver"
        for alias in node.names
    }
    errors = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        parts = ast.unparse(node.func).split(".")
        if parts[0] not in imported:
            continue
        name = ".".join([imported[parts[0]], *parts[1:]])
        sig = signatures.get(name)
        if sig is None:
            continue
        given = {k.arg for k in node.keywords}
        if None in given:
            continue  # **kwargs: cannot tell statically
        npos = len(node.args)
        missing = [p for p in sig.positional[npos : sig.required_positional] if p not in given]
        missing += sorted(sig.required_keywords - given)
        unknown = sorted(given - set(sig.positional) - sig.keywords)
        problems = []
        if npos > len(sig.positional):
            problems.append(f"{npos} positional arguments, accepts {len(sig.positional)}")
        if missing:
            problems.append(f"missing {', '.join(missing)}")
        if unknown:
            problems.append(f"unexpected {', '.join(unknown)}")
        if problems:
            errors.append(f"line {node.lineno}: {name}(...) {'; '.join(problems)}")
    return errors


def driver_blocks() -> list[tuple[str, int, str]]:
    """(repo path, line, code) for every Python fence that imports from cua_driver."""
    blocks = []
    for root, pattern in SOURCES:
        for path in sorted(root.rglob(pattern)):
            if "node_modules" in path.parts:
                continue
            text = path.read_text(encoding="utf-8")
            for m in FENCE.finditer(text):
                if re.search(r"^\s*from cua_driver import", m.group(1), re.M):
                    line = text.count("\n", 0, m.start()) + 1
                    blocks.append((str(path.relative_to(REPO)), line, m.group(1)))
    return blocks


class DriverConstructorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.signatures = contract_signatures(CONTRACT.read_text(encoding="utf-8"))

    def test_contract_parses(self) -> None:
        self.assertIn("GetWindowStateInput", self.signatures)
        self.assertIn("ActionTarget.WINDOW", self.signatures)

    def test_docs_blocks_construct_against_generated_bindings(self) -> None:
        blocks = driver_blocks()
        self.assertTrue(
            any(p.endswith("cua-driver/guides/use-the-sdk.mdx") for p, _, _ in blocks),
            "the Use the SDK guide's Python example was not found",
        )
        failures = []
        for path, line, code in blocks:
            try:
                errors = call_errors(code, self.signatures)
            except SyntaxError as e:
                errors = [f"does not parse: {e}"]
            failures += [f"{path}:{line}: {err}" for err in errors]
        self.assertEqual(failures, [], "\n".join(failures))

    def test_detects_a_missing_required_optional_field(self) -> None:
        block = (
            "from cua_driver import GetWindowStateInput as G, ActionTarget\n"
            "G(pid=1, window_id=1, bogus=2)\n"
            "ActionTarget.WINDOW(1)\n"
        )
        errors = call_errors(block, self.signatures)
        self.assertEqual(len(errors), 2, errors)
        self.assertIn("missing", errors[0])
        self.assertIn("max_image_dimension", errors[0])
        self.assertIn("unexpected bogus", errors[0])
        self.assertIn("ActionTarget.WINDOW(...) missing window_id", errors[1])


if __name__ == "__main__":
    unittest.main()

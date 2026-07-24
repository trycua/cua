#!/usr/bin/env python3
"""Generate the public ``cua-sandbox`` Python API reference from source."""

from __future__ import annotations

import argparse
import ast
import difflib
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE_DIR = ROOT / "libs/python/cua-sandbox/cua_sandbox"
PACKAGE_INIT = PACKAGE_DIR / "__init__.py"
OUTPUT = ROOT / "docs/content/docs/reference/sandbox-sdk/index.mdx"
DETAILED_CLASSES = ("Sandbox", "Image", "Localhost")


@dataclass(frozen=True)
class Export:
    name: str
    module: str
    source_name: str


def parse_file(path: Path) -> ast.Module:
    return ast.parse(path.read_text(), filename=str(path))


def module_path(module: str) -> Path:
    relative_module = module.removeprefix("cua_sandbox.")
    return PACKAGE_DIR / (relative_module.replace(".", "/") + ".py")


def public_exports() -> list[Export]:
    tree = parse_file(PACKAGE_INIT)
    imports: dict[str, Export] = {}
    exports: list[str] | None = None
    for node in tree.body:
        if isinstance(node, ast.ImportFrom) and node.module:
            for alias in node.names:
                imports[alias.asname or alias.name] = Export(
                    alias.asname or alias.name, node.module, alias.name
                )
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "__all__" for target in node.targets
        ):
            exports = ast.literal_eval(node.value)
    if exports is None:
        raise RuntimeError("cua_sandbox.__all__ is required for public API generation")
    return [imports[name] for name in exports]


def definition(module: str, name: str) -> ast.ClassDef | ast.FunctionDef | ast.AsyncFunctionDef:
    for node in parse_file(module_path(module)).body:
        if (
            isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef))
            and node.name == name
        ):
            return node
    raise RuntimeError(f"Could not find {name} in cua_sandbox.{module}")


def annotation(node: ast.expr | None) -> str:
    return ast.unparse(node) if node is not None else "Any"


def format_argument(argument: ast.arg, default: ast.expr | None = None) -> str:
    result = argument.arg
    if argument.annotation is not None:
        result += f": {annotation(argument.annotation)}"
    if default is not None:
        result += f" = {ast.unparse(default)}"
    return result


def signature(node: ast.FunctionDef | ast.AsyncFunctionDef, *, include_return: bool = True) -> str:
    positional = [*node.args.posonlyargs, *node.args.args]
    defaults = [None] * (len(positional) - len(node.args.defaults)) + list(node.args.defaults)
    parts = [format_argument(argument, default) for argument, default in zip(positional, defaults)]
    if node.args.posonlyargs:
        parts.insert(len(node.args.posonlyargs), "/")
    if node.args.vararg is not None:
        parts.append("*" + format_argument(node.args.vararg))
    elif node.args.kwonlyargs:
        parts.append("*")
    for argument, default in zip(node.args.kwonlyargs, node.args.kw_defaults):
        parts.append(format_argument(argument, default))
    if node.args.kwarg is not None:
        parts.append("**" + format_argument(node.args.kwarg))
    prefix = "async def" if isinstance(node, ast.AsyncFunctionDef) else "def"
    result = f"{prefix} {node.name}({', '.join(parts)})"
    if include_return and node.returns is not None:
        result += f" -> {annotation(node.returns)}"
    return result


def summary(node: ast.AST) -> str:
    docstring = ast.get_docstring(node) or ""
    return docstring.strip().split("\n\n", 1)[0].replace("\n", " ")


def raised_exceptions(node: ast.AST) -> list[str]:
    exceptions: set[str] = set()
    for child in ast.walk(node):
        if not isinstance(child, ast.Raise) or child.exc is None:
            continue
        exc = child.exc.func if isinstance(child.exc, ast.Call) else child.exc
        if isinstance(exc, ast.Name):
            exceptions.add(exc.id)
        elif isinstance(exc, ast.Attribute):
            exceptions.add(exc.attr)
    return sorted(exceptions)


def public_methods(node: ast.ClassDef) -> list[ast.FunctionDef | ast.AsyncFunctionDef]:
    return [
        child
        for child in node.body
        if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef))
        and not child.name.startswith("_")
    ]


def render_function(
    node: ast.FunctionDef | ast.AsyncFunctionDef, heading: str, *, include_return: bool = True
) -> list[str]:
    lines = [
        f"#### `{heading}`",
        "",
        "```python",
        signature(node, include_return=include_return),
        "```",
        "",
    ]
    if description := summary(node):
        lines.extend([description, ""])
    if exceptions := raised_exceptions(node):
        lines.extend([f"Raises: {', '.join(f'`{exc}`' for exc in exceptions)}.", ""])
    return lines


def render_class(export: Export) -> list[str]:
    node = definition(export.module, export.source_name)
    assert isinstance(node, ast.ClassDef)
    lines = [f"## `{export.name}`", ""]
    if description := summary(node):
        lines.extend([description, ""])
    if export.name not in DETAILED_CLASSES:
        return [*lines, "This public type is exported from `cua_sandbox`.", ""]
    if export.name == "Sandbox":
        lines.extend(
            [
                "`Sandbox` is an async context manager: leaving `async with` disconnects the client but leaves the sandbox running. `Sandbox.ephemeral` destroys the sandbox when its context exits.",
                "",
            ]
        )
    if export.name == "Localhost":
        lines.extend(
            [
                "`Localhost` is an async context manager; leaving its context disconnects the client.",
                "",
            ]
        )
    for method in public_methods(node):
        private_return = method.returns is not None and "_" in annotation(method.returns)
        lines.extend(
            render_function(
                method, f"{export.name}.{method.name}", include_return=not private_return
            )
        )
        if export.name in {"Sandbox", "Localhost"} and method.name == "connect":
            lines.extend(
                [
                    f"Returns a `{export.name}` when awaited, or acts as an async context manager that disconnects on exit.",
                    "",
                ]
            )
    return lines


def render_reference() -> str:
    exports = public_exports()
    lines = [
        "---",
        "title: Sandbox SDK reference",
        'description: "Generated reference for the public cua-sandbox Python API."',
        "---",
        "",
        "{/* Generated by: python3 scripts/docs-generators/generate_sandbox_sdk_reference.py. Do not edit directly. */}",
        "",
        "# Sandbox SDK reference",
        "",
        "Install `cua-sandbox` and import it as `cua_sandbox` (often aliased to `cua`).",
        "",
        "```bash",
        "pip install cua-sandbox",
        "```",
        "",
        "## Public exports",
        "",
        "The package's supported public surface is defined by `cua_sandbox.__all__`.",
        "",
        "| Export | Kind |",
        "| --- | --- |",
    ]
    for export in exports:
        node = definition(export.module, export.source_name)
        lines.append(
            f"| `{export.name}` | {'class' if isinstance(node, ast.ClassDef) else 'function'} |"
        )
    lines.append("")
    for export in exports:
        node = definition(export.module, export.source_name)
        lines.extend(
            render_class(export)
            if isinstance(node, ast.ClassDef)
            else render_function(node, export.name)
        )
    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="Fail when the checked-in reference is stale."
    )
    args = parser.parse_args()
    generated = render_reference()
    existing = OUTPUT.read_text() if OUTPUT.exists() else ""
    if args.check:
        if existing == generated:
            print(f"{OUTPUT.relative_to(ROOT)} is current")
            return 0
        print(
            f"{OUTPUT.relative_to(ROOT)} is stale; run python3 scripts/docs-generators/generate_sandbox_sdk_reference.py",
            file=sys.stderr,
        )
        print(
            "\n".join(
                difflib.unified_diff(
                    existing.splitlines(),
                    generated.splitlines(),
                    fromfile="checked-in",
                    tofile="generated",
                    lineterm="",
                )
            ),
            file=sys.stderr,
        )
        return 1
    OUTPUT.write_text(generated)
    print(f"Wrote {OUTPUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

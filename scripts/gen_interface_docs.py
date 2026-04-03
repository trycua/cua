#!/usr/bin/env python3
"""Generate docs/content/docs/cua/reference/sandbox-sdk/interfaces.mdx
from the pydocstrings in cua_sandbox/interfaces/*.py.

Usage:
    python scripts/gen_interface_docs.py
    python scripts/gen_interface_docs.py --dry-run   # print to stdout only

The script introspects each interface module, collects class/method
docstrings + signatures, and emits structured MDX.
"""

from __future__ import annotations

import argparse
import ast
import inspect
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
INTERFACES_DIR = REPO_ROOT / "libs/python/cua-sandbox/cua_sandbox/interfaces"
OUTPUT_FILE = REPO_ROOT / "docs/content/docs/cua/reference/sandbox-sdk/interfaces.mdx"

# Order in which classes appear in the output
CLASS_ORDER = [
    "Shell",
    "CommandResult",
    "Mouse",
    "Keyboard",
    "Screen",
    "Clipboard",
    "Tunnel",
    "TunnelInfo",
    "Terminal",
    "Window",
    "Mobile",
]

# Module file for each class
CLASS_MODULE = {
    "Shell": "shell",
    "CommandResult": "shell",
    "Mouse": "mouse",
    "Keyboard": "keyboard",
    "Screen": "screen",
    "Clipboard": "clipboard",
    "Tunnel": "tunnel",
    "TunnelInfo": "tunnel",
    "Terminal": "terminal",
    "Window": "window",
    "Mobile": "mobile",
}

# ── AST helpers ──────────────────────────────────────────────────────────────


def _parse_module(path: Path) -> ast.Module:
    return ast.parse(path.read_text())


def _get_docstring(node) -> str:
    ds = ast.get_docstring(node) or ""
    return textwrap.dedent(ds).strip()


def _format_arg(arg: ast.arg, defaults: dict[str, ast.expr]) -> str:
    name = arg.arg
    annotation = ""
    if arg.annotation:
        annotation = f": {ast.unparse(arg.annotation)}"
    default = ""
    if name in defaults:
        default = f" = {ast.unparse(defaults[name])}"
    return f"{name}{annotation}{default}"


def _method_signature(func: ast.FunctionDef) -> str:
    """Return a human-readable signature string (without 'self')."""
    args = func.args
    # Build default map: last N positional args get the last N defaults
    all_args = args.args
    defaults = {}
    n_defaults = len(args.defaults)
    if n_defaults:
        for arg, default in zip(all_args[-n_defaults:], args.defaults):
            defaults[arg.arg] = default

    # kwonly defaults
    for arg, default in zip(args.kwonlyargs, args.kw_defaults):
        if default is not None:
            defaults[arg.arg] = default

    parts = []
    for arg in all_args:
        if arg.arg == "self":
            continue
        parts.append(_format_arg(arg, defaults))

    for arg in args.kwonlyargs:
        parts.append(_format_arg(arg, defaults))

    if args.vararg:
        parts.append(f"*{args.vararg.arg}")
    if args.kwarg:
        parts.append(f"**{args.kwarg.arg}")

    ret = ""
    if func.returns:
        ret = f" -> {ast.unparse(func.returns)}"

    return f"({', '.join(parts)}){ret}"


def _is_public(name: str) -> bool:
    return not name.startswith("_")


# ── Collect info ─────────────────────────────────────────────────────────────


class FieldInfo:
    def __init__(self, name: str, annotation: str):
        self.name = name
        self.annotation = annotation


class MethodInfo:
    def __init__(self, name: str, sig: str, doc: str, is_async: bool, is_property: bool):
        self.name = name
        self.sig = sig
        self.doc = doc
        self.is_async = is_async
        self.is_property = is_property


class ClassInfo:
    def __init__(self, name: str, doc: str, fields: list[FieldInfo], methods: list[MethodInfo]):
        self.name = name
        self.doc = doc
        self.fields = fields
        self.methods = methods


def _collect_class(tree: ast.Module, class_name: str) -> ClassInfo | None:
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef) or node.name != class_name:
            continue
        doc = _get_docstring(node)
        fields: list[FieldInfo] = []
        methods: list[MethodInfo] = []
        for item in node.body:
            # Dataclass-style annotated field: `name: type` or `name: type = value`
            if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                fname = item.target.id
                if _is_public(fname):
                    ann = ast.unparse(item.annotation) if item.annotation else ""
                    fields.append(FieldInfo(name=fname, annotation=ann))
            elif isinstance(item, ast.FunctionDef | ast.AsyncFunctionDef):
                if not _is_public(item.name):
                    continue
                is_property = any(
                    (isinstance(d, ast.Name) and d.id == "property")
                    or (isinstance(d, ast.Attribute) and d.attr == "property")
                    for d in item.decorator_list
                )
                methods.append(
                    MethodInfo(
                        name=item.name,
                        sig=_method_signature(item),
                        doc=_get_docstring(item),
                        is_async=isinstance(item, ast.AsyncFunctionDef),
                        is_property=is_property,
                    )
                )
        return ClassInfo(name=class_name, doc=doc, fields=fields, methods=methods)
    return None


def collect_all() -> list[ClassInfo]:
    infos = []
    for class_name in CLASS_ORDER:
        mod_name = CLASS_MODULE[class_name]
        path = INTERFACES_DIR / f"{mod_name}.py"
        if not path.exists():
            print(f"  [skip] {path} not found")
            continue
        tree = _parse_module(path)
        info = _collect_class(tree, class_name)
        if info:
            infos.append(info)
        else:
            print(f"  [warn] class {class_name} not found in {path}")
    return infos


# ── MDX rendering ─────────────────────────────────────────────────────────────


def _escape_mdx(text: str) -> str:
    """Escape <> in doc text so MDX doesn't treat them as JSX tags.

    Preserves content inside backtick code spans and fenced code blocks.
    """
    import re

    # Split on backtick-delimited segments (`` ` `` or ``` `` ```)
    # Odd-indexed segments are inside backticks — leave them alone
    parts = re.split(r"(```.+?```|``.+?``|`.+?`)", text, flags=re.DOTALL)
    for i, part in enumerate(parts):
        if i % 2 == 0:  # outside code
            parts[i] = (
                part.replace("<", "&lt;")
                .replace(">", "&gt;")
                .replace("{", "&#123;")
                .replace("}", "&#125;")
            )
    return "".join(parts)


def _render_method(m: MethodInfo) -> str:
    lines = []
    prefix = "async " if m.is_async else ""
    prop = "@property\n" if m.is_property else ""
    lines.append(f"#### `{prop}{prefix}{m.name}{m.sig}`\n")
    if m.doc:
        lines.append(_escape_mdx(m.doc))
        lines.append("")
    return "\n".join(lines)


def _render_class(c: ClassInfo) -> str:
    lines = []
    lines.append(f"## `{c.name}`\n")
    if c.doc:
        lines.append(_escape_mdx(c.doc))
        lines.append("")

    if c.fields:
        lines.append("### Fields\n")
        lines.append("| Name | Type |")
        lines.append("|------|------|")
        for f in c.fields:
            lines.append(f"| `{f.name}` | `{_escape_mdx(f.annotation)}` |")
        lines.append("")

    if c.methods:
        lines.append("### Methods\n")
        for m in c.methods:
            lines.append(_render_method(m))

    return "\n".join(lines)


HEADER = """\
---
title: Interfaces Reference
description: Auto-generated reference for all cua-sandbox interface classes. Do not edit manually — run scripts/gen_interface_docs.py to regenerate.
---

import { Callout } from "fumadocs-ui/components/callout";

<Callout type="info">
  This page is auto-generated from the Python source docstrings.
  Run `python scripts/gen_interface_docs.py` to regenerate after editing interface code.
</Callout>

The sandbox exposes the following interface objects on every `Sandbox` instance:

| Attribute | Class | Purpose |
|-----------|-------|---------|
| `sb.shell` | `Shell` | Run shell commands |
| `sb.mouse` | `Mouse` | Mouse control |
| `sb.keyboard` | `Keyboard` | Keyboard control |
| `sb.screen` | `Screen` | Screenshots and screen info |
| `sb.clipboard` | `Clipboard` | Clipboard read/write |
| `sb.tunnel` | `Tunnel` | Port forwarding |
| `sb.terminal` | `Terminal` | PTY terminal sessions |
| `sb.window` | `Window` | Window management |
| `sb.mobile` | `Mobile` | Mobile-specific actions |

---

"""


def render(infos: list[ClassInfo]) -> str:
    parts = [HEADER]
    for info in infos:
        parts.append(_render_class(info))
        parts.append("\n---\n\n")
    return "\n".join(parts)


# ── Main ──────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print output to stdout instead of writing to file",
    )
    args = parser.parse_args()

    print(f"Collecting interfaces from {INTERFACES_DIR} ...")
    infos = collect_all()
    print(f"  Found {len(infos)} classes: {[c.name for c in infos]}")

    content = render(infos)

    if args.dry_run:
        print("\n" + "=" * 60 + "\n")
        print(content)
    else:
        OUTPUT_FILE.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT_FILE.write_text(content)
        print(f"Written to {OUTPUT_FILE}")


if __name__ == "__main__":
    main()

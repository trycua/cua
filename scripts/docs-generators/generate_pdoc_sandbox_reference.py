#!/usr/bin/env python3
"""Generate the public ``cua-sandbox`` API reference with pdoc."""

from __future__ import annotations

import argparse
import ast
import difflib
import inspect
import re
import subprocess
import sys
import textwrap
import types
import warnings
from pathlib import Path

from pdoc.doc import Class, Doc, Function, Module, Variable, empty

ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs"
PACKAGE_PARENT = ROOT / "libs/python/cua-sandbox"
OUTPUT = DOCS / "content/docs/reference/sandbox-sdk/index.mdx"
PRETTIER = DOCS / "node_modules/.bin/prettier"
EXCLUDED_MODULE_PARTS = (".transport.fleet", "cyclops_sdk")


def load_pdoc_module(name: str) -> Module:
    """Load a module while retaining source-form annotations pdoc cannot resolve."""
    with warnings.catch_warnings():
        warnings.filterwarnings(
            "ignore",
            message=r"Error parsing type annotation .*",
            category=UserWarning,
            module=r"pdoc\.doc_types",
        )
        return Module.from_name(name)


def install_fleet_import_stub() -> None:
    """Avoid requiring Fleet's native binding for excluded documentation."""
    if "cyclops_sdk" in sys.modules:
        return

    binding_stub = types.ModuleType("cyclops_sdk")

    def placeholder(name: str) -> type[object]:
        value = type(name, (), {})
        setattr(binding_stub, name, value)
        return value

    binding_stub.__getattr__ = placeholder
    sys.modules["cyclops_sdk"] = binding_stub


def load_public_module() -> Module:
    """Load the installed source package through pdoc's supported API."""
    package_parent = str(PACKAGE_PARENT)
    if package_parent not in sys.path:
        sys.path.insert(0, package_parent)
    install_fleet_import_stub()
    return load_pdoc_module("cua_sandbox")


def public_members(module: Module) -> list[Doc]:
    """Resolve the root package's explicit public API in declaration order."""
    exports = getattr(module.obj, "__all__", None)
    if not isinstance(exports, list) or not all(isinstance(name, str) for name in exports):
        raise RuntimeError("cua_sandbox.__all__ must be a list of public export names")

    missing = [name for name in exports if name not in module.members]
    if missing:
        raise RuntimeError(f"pdoc could not resolve cua_sandbox exports: {', '.join(missing)}")
    return [module.members[name] for name in exports]


def public_interface_members(module: Module) -> list[Doc]:
    """Resolve the separately exported public interface handles."""
    interfaces = load_pdoc_module(f"{module.modulename}.interfaces")
    exports = getattr(interfaces.obj, "__all__", None)
    if not isinstance(exports, list) or not all(isinstance(name, str) for name in exports):
        raise RuntimeError(f"{interfaces.modulename}.__all__ must be a list of public export names")

    missing = [name for name in exports if name not in interfaces.members]
    if missing:
        raise RuntimeError(
            f"pdoc could not resolve {interfaces.modulename} exports: {', '.join(missing)}"
        )
    return [interfaces.members[name] for name in exports]


def is_documentable(member: Doc) -> bool:
    """Keep pdoc's public members while excluding internal transport implementation."""
    return not (
        member.name.startswith("_")
        # ``send`` is the low-level action dispatcher on transport implementations.
        or member.name == "send"
        or any(part in member.modulename for part in EXCLUDED_MODULE_PARTS)
        or any(part in member.fullname for part in EXCLUDED_MODULE_PARTS)
    )


def normalize_annotation(value: str, member: Doc) -> str:
    """Remove internal package qualification from pdoc's display annotations."""
    root_package = member.modulename.partition(".")[0]
    return re.sub(rf"\b{re.escape(root_package)}(?:\.[a-z_]\w*)*\.", "", value)


def signature(member: Function) -> str:
    """Format pdoc's normalized signature as a Python declaration."""
    declaration = re.search(r"^\s*(async\s+)?def\s", member.source, re.MULTILINE)
    prefix = "async def" if declaration and declaration.group(1) else "def"
    return normalize_annotation(f"{prefix} {member.name}{member.signature}", member)


def class_signature(member: Class) -> str:
    return normalize_annotation(f"class {member.name}{inspect.signature(member.obj)}", member)


def mdx_docstring(docstring: str) -> str:
    """Escape MDX expressions in prose without changing fenced examples."""
    in_fence = False
    escaped_lines = []
    for line in docstring.splitlines():
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
        if not in_fence:
            line = (
                line.replace("{", "\\{")
                .replace("}", "\\}")
                .replace("<", "&lt;")
                .replace(">", "&gt;")
            )
        escaped_lines.append(line)
    return "\n".join(escaped_lines)


def append_docstring(lines: list[str], member: Doc) -> None:
    docstring = member.docstring.strip()
    if docstring:
        lines.extend((mdx_docstring(docstring), ""))


def raised_exceptions(member: Function) -> list[str]:
    """List direct exception types from pdoc's live public function object."""
    try:
        tree = ast.parse(textwrap.dedent(inspect.getsource(member.obj)))
    except (OSError, SyntaxError, TypeError):
        return []

    exceptions: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Raise) or node.exc is None:
            continue
        exception = node.exc.func if isinstance(node.exc, ast.Call) else node.exc
        if isinstance(exception, ast.Name):
            exceptions.add(exception.id)
        elif isinstance(exception, ast.Attribute):
            exceptions.add(exception.attr)
    return sorted(exceptions)


def render_function(lines: list[str], member: Function, level: str) -> None:
    lines.extend((f"{level} `{member.fullname}`", "", "```python", signature(member), "```", ""))
    append_docstring(lines, member)
    if exceptions := raised_exceptions(member):
        lines.extend((f"Raises: {', '.join(f'`{exception}`' for exception in exceptions)}", ""))


def render_variable(lines: list[str], member: Variable, level: str) -> None:
    annotation_value = member.annotation
    if annotation_value is empty:
        annotation_value = None
    elif isinstance(annotation_value, type):
        annotation_value = annotation_value.__name__
    annotation = (
        f": {normalize_annotation(str(annotation_value).replace('typing.', ''), member)}"
        if annotation_value
        else ""
    )
    lines.extend(
        (f"{level} `{member.fullname}`", "", "```python", f"{member.name}{annotation}", "```", "")
    )
    append_docstring(lines, member)


def render_class(lines: list[str], member: Class, level: str = "##") -> None:
    lines.extend((f"{level} `{member.name}`", "", "```python", class_signature(member), "```", ""))
    append_docstring(lines, member)
    for child in member.own_members:
        if not is_documentable(child):
            continue
        if isinstance(child, Function):
            render_function(lines, child, "###")
        elif isinstance(child, Variable):
            render_variable(lines, child, "###")


def render_reference(module: Module) -> str:
    """Render root exports and their public members as deterministic Fumadocs MDX."""
    lines = [
        "---",
        "title: Sandbox SDK API Reference",
        "description: Public Python API for cua-sandbox.",
        "---",
        "",
        "{/* Generated by scripts/docs-generators/generate_pdoc_sandbox_reference.py. */}",
        "",
        "This reference is generated from `cua_sandbox.__all__` using pdoc. "
        "pdoc imports `cua_sandbox` source at generation time; it does not statically analyze "
        "the package. Excluded native Fleet bindings are stubbed rather than loaded.",
        "",
        "```python",
        "import cua_sandbox",
        "```",
        "",
    ]
    for member in public_members(module):
        if not is_documentable(member):
            continue
        if isinstance(member, Class):
            render_class(lines, member)
        elif isinstance(member, Function):
            render_function(lines, member, "##")
        elif isinstance(member, Variable):
            render_variable(lines, member, "##")
    interface_members = public_interface_members(module)
    if interface_members:
        lines.extend(
            ("## Interfaces", "", "Public service, tunnel, and computer-control handles.", "")
        )
    for member in interface_members:
        if not is_documentable(member):
            continue
        if isinstance(member, Class):
            render_class(lines, member, "###")
        elif isinstance(member, Function):
            render_function(lines, member, "###")
        elif isinstance(member, Variable):
            render_variable(lines, member, "###")
    return "\n".join(lines).rstrip() + "\n"


def format_mdx(reference: str) -> str:
    """Format generated MDX through the lockfile-pinned docs formatter."""
    try:
        result = subprocess.run(
            [str(PRETTIER), "--stdin-filepath", str(OUTPUT.relative_to(DOCS))],
            check=False,
            cwd=DOCS,
            input=reference,
            stderr=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
    except FileNotFoundError as error:
        raise RuntimeError(
            "install docs dependencies before generating the sandbox reference"
        ) from error
    if result.returncode:
        raise RuntimeError(f"Prettier failed to format sandbox reference:\n{result.stderr}")
    return result.stdout


def check_output(expected: str, output: Path = OUTPUT) -> bool:
    actual = output.read_text() if output.exists() else ""
    if actual == expected:
        return True
    diff = difflib.unified_diff(
        actual.splitlines(keepends=True),
        expected.splitlines(keepends=True),
        fromfile=str(output),
        tofile="generated",
    )
    sys.stderr.writelines(diff)
    return False


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if the committed MDX is stale")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    reference = format_mdx(render_reference(load_public_module()))
    if args.check:
        return 0 if check_output(reference) else 1
    OUTPUT.write_text(reference)
    print(f"Wrote {OUTPUT.relative_to(ROOT)} ({len(reference.encode())} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

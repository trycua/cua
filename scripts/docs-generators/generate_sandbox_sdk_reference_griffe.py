#!/usr/bin/env python3
"""Generate the public ``cua-sandbox`` API reference with Griffe."""

from __future__ import annotations

import argparse
import difflib
from pathlib import Path
from typing import Any

from griffe import load

ROOT = Path(__file__).resolve().parents[2]
PACKAGE_ROOT = ROOT / "libs/python/cua-sandbox"
OUTPUT = ROOT / "docs/content/docs/reference/sandbox-sdk/index.mdx"

CONFIG_FUNCTIONS = ("configure", "login", "whoami")
LIFECYCLE_METHODS = (
    "create",
    "connect",
    "ephemeral",
    "disconnect",
    "destroy",
    "list",
    "get_info",
    "suspend",
    "resume",
    "restart",
    "delete",
)
FORBIDDEN_PATH_PARTS = ("cyclops_sdk", ".transport.fleet")
FORBIDDEN_MEMBERS = {"send", "raw_request", "request"}
PUBLIC_RETURN_TYPES = {
    ("Sandbox", "connect"): "Sandbox",
    ("Sandbox", "ephemeral"): "Sandbox",
    ("Localhost", "connect"): "Localhost",
    ("Tunnel", "forward"): "TunnelInfo | dict[int | str, TunnelInfo]",
}
SEMANTIC_NOTES = {
    ("Sandbox", "connect"): (
        "Supports both `await` and `async with`; leaving the context calls "
        "`disconnect()` without destroying the sandbox."
    ),
    ("Sandbox", "get_info"): "Raises: `ValueError` when a requested local sandbox is not found.",
    (
        "Sandbox",
        "resume",
    ): "Raises: `ValueError` when local state is missing or has an unknown runtime.",
    ("Sandbox", "delete"): "Raises: `ValueError` when a requested local sandbox is not found.",
    ("Image", "macos"): "Raises: `ValueError` for an unsupported macOS version.",
    (
        "Image",
        "pwa_install",
    ): "Raises: `ValueError` for an unsupported builder or non-Android image.",
    (
        "Localhost",
        "connect",
    ): "Supports both `await` and `async with`; exit disconnects from the host.",
    ("Tunnel", "forward"): "Raises: `ValueError` when no port or socket name is supplied.",
}


class ReferenceError(RuntimeError):
    """Raised when the package public API cannot be rendered safely."""


class StaleReferenceError(ReferenceError):
    """Raised when checked-in MDX does not match the generated reference."""


def load_package(module_name: str = "cua_sandbox") -> Any:
    """Load source definitions without importing the package at runtime."""
    return load(module_name, search_paths=[PACKAGE_ROOT])


def resolved(member: Any) -> Any:
    """Resolve imports and re-exports through Griffe aliases."""
    return member.final_target if member.is_alias else member


def unquote_literal(value: str) -> str:
    """Normalize a string literal represented by Griffe's expression model."""
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


def public_names(module: Any) -> tuple[str, ...]:
    """Read and validate the public contract from Griffe's parsed ``__all__``."""
    if "__all__" not in module.members:
        raise ReferenceError("cua_sandbox.__all__ is required for reference generation")
    elements = getattr(module.members["__all__"].value, "elements", None)
    if not isinstance(elements, list) or not all(isinstance(name, str) for name in elements):
        raise ReferenceError("cua_sandbox.__all__ must be a literal list of names")
    names = tuple(unquote_literal(name) for name in elements)
    if len(names) != len(set(names)):
        raise ReferenceError("cua_sandbox.__all__ must not contain duplicate names")
    missing = [name for name in names if name not in module.members]
    if missing:
        raise ReferenceError(f"cua_sandbox.__all__ exports missing members: {', '.join(missing)}")
    return names


def assert_allowed(member: Any) -> Any:
    """Keep private transport implementations and raw operations out of docs."""
    target = resolved(member)
    path = target.canonical_path
    if any(part in path for part in FORBIDDEN_PATH_PARTS) or target.name in FORBIDDEN_MEMBERS:
        raise ReferenceError(f"forbidden API surface: {path}")
    return target


def summary(member: Any) -> str:
    """Return Griffe's source docstring summary, when one exists."""
    if not member.docstring:
        return ""
    return (
        member.docstring.value.strip()
        .split("\n\n", 1)[0]
        .replace("\n", " ")
        .replace("``", "`")
        .replace("{", "\\{")
        .replace("}", "\\}")
    )


def markdown_table(headers: tuple[str, ...], rows: list[tuple[str, ...]]) -> list[str]:
    """Render a stable Markdown table that is already Prettier-formatted."""
    widths = [
        max(3, len(header), *(len(row[index]) for row in rows))
        for index, header in enumerate(headers)
    ]

    def table_row(values: tuple[str, ...]) -> str:
        return (
            "| "
            + " | ".join(value.ljust(widths[index]) for index, value in enumerate(values))
            + " |"
        )

    return [
        table_row(headers),
        table_row(tuple("-" * width for width in widths)),
        *(table_row(row) for row in rows),
    ]


def signature(member: Any, owner: str) -> str:
    """Render Griffe's source-derived signature with public return semantics."""
    source_signature = str(member.signature())
    if public_return := PUBLIC_RETURN_TYPES.get((owner, member.name)):
        source_signature = source_signature.rsplit(" -> ", 1)[0] + f" -> {public_return}"
    prefix = "async def" if "async" in member.labels else "def"
    return f"{prefix} {source_signature}"


def render_callable(member: Any, heading: str, owner: str) -> list[str]:
    """Render a function or method from its Griffe definition."""
    lines = [f"#### `{heading}`", "", "```python", signature(member, owner), "```", ""]
    if text := summary(member):
        lines.extend([text, ""])
    if note := SEMANTIC_NOTES.get((owner, member.name)):
        lines.extend([note, ""])
    return lines


def render_class(package: Any, name: str) -> list[str]:
    """Render a public class and its public, non-raw methods."""
    target = assert_allowed(package.members[name])
    lines = [f"## `{name}`", ""]
    if text := summary(target):
        lines.extend([text, ""])
    if name == "Sandbox":
        return lines
    for method_name, method_member in target.members.items():
        if method_name.startswith("_") or method_name in FORBIDDEN_MEMBERS:
            continue
        method = assert_allowed(method_member)
        if method.is_function:
            lines.extend(render_callable(method, f"{name}.{method_name}", name))
    return lines


def render_config(package: Any, exports: tuple[str, ...]) -> list[str]:
    """Render authentication and configuration functions exported by the package."""
    lines = ["## Configuration and authentication", ""]
    for name in CONFIG_FUNCTIONS:
        if name not in exports:
            raise ReferenceError(f"cua_sandbox.__all__ is missing required export {name}")
        lines.extend(render_callable(assert_allowed(package.members[name]), name, name))
    return lines


def render_lifecycle(package: Any) -> list[str]:
    """Render the lifecycle contract for persistent and ephemeral sandboxes."""
    sandbox = assert_allowed(package.members["Sandbox"])
    lines = [
        "## Lifecycle and service access",
        "",
        "Use `Sandbox.create()` to provision, `connect()` to attach, and `destroy()` to delete.",
        "",
    ]
    for name in LIFECYCLE_METHODS:
        lines.extend(
            render_callable(assert_allowed(sandbox.members[name]), f"Sandbox.{name}", "Sandbox")
        )
    return lines


def render_interfaces() -> list[str]:
    """Render curated service interfaces, excluding transport implementations."""
    interfaces = load_package("cua_sandbox.interfaces")
    lines = ["## Tunnels and exported interfaces", ""]
    for name in public_names(interfaces):
        target = assert_allowed(interfaces.members[name])
        if not target.is_class:
            continue
        lines.extend([f"### `{name}`", ""])
        if text := summary(target):
            lines.extend([text, ""])
        for method_name, method_member in target.members.items():
            if method_name.startswith("_") or method_name in FORBIDDEN_MEMBERS:
                continue
            method = assert_allowed(method_member)
            if method.is_function:
                lines.extend(render_callable(method, f"{name}.{method_name}", name))
    return lines


def render_exports(package: Any, exports: tuple[str, ...]) -> list[str]:
    """Render the explicit package contract, not every importable implementation."""
    lines = [
        "## Public exports",
        "",
        "The supported public surface is defined by `cua_sandbox.__all__`.",
        "",
    ]
    rows = [(f"`{name}`", assert_allowed(package.members[name]).kind.value) for name in exports]
    return [*lines, *markdown_table(("Export", "Kind"), rows), ""]


def render_reference() -> str:
    """Create deterministic Fumadocs MDX from the public Griffe model."""
    package = load_package()
    exports = public_names(package)
    lines = [
        "---",
        "title: Sandbox SDK reference",
        "description: Generated reference for the public cua-sandbox Python API.",
        "---",
        "",
        "{/* Generated by: uv run --group docs-scripts python scripts/docs-generators/generate_sandbox_sdk_reference_griffe.py. Do not edit directly. */}",
        "",
        "Install `cua-sandbox` and import it as `cua_sandbox` (often aliased to `cua`).",
        "",
        "```bash",
        "pip install cua-sandbox",
        "```",
        "",
    ]
    lines.extend(render_exports(package, exports))
    lines.extend(render_config(package, exports))
    for name in ("Sandbox", "Image", "Localhost"):
        lines.extend(render_class(package, name))
    lines.extend(render_lifecycle(package))
    lines.extend(render_interfaces())
    return "\n".join(lines).rstrip() + "\n"


def write_reference(output: Path = OUTPUT) -> None:
    """Write the generated reference to its Fumadocs source path."""
    output.write_text(render_reference())


def check_reference(output: Path = OUTPUT) -> None:
    """Fail when checked-in MDX differs from deterministic generated output."""
    expected = render_reference()
    actual = output.read_text() if output.exists() else ""
    if actual == expected:
        return
    diff = "".join(
        difflib.unified_diff(
            actual.splitlines(keepends=True),
            expected.splitlines(keepends=True),
            fromfile=str(output),
            tofile=f"generated {output}",
        )
    )
    raise StaleReferenceError(f"Sandbox SDK reference is stale. Run the generator.\n{diff}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if generated MDX is stale")
    parser.add_argument("--output", type=Path, default=OUTPUT, help="write a different MDX path")
    args = parser.parse_args()
    if args.check:
        check_reference(args.output)
    else:
        write_reference(args.output)


if __name__ == "__main__":
    main()

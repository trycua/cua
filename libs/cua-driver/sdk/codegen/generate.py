#!/usr/bin/env python3
"""generate.py — Generate typed Python bindings from a live cua-driver MCP schema.

Launches ``cua-driver mcp``, calls ``tools/list``, then writes a Python
module with one typed method per tool.  Run this whenever the Swift tool
definitions change to keep the generated bindings in sync.

Usage
-----
::

    # Default: binary auto-detected, output to stdout
    python generate.py

    # Explicit binary, write to file
    python generate.py --binary .build/release/cua-driver --output generated_tools.py

    # Also print the raw schema JSON
    python generate.py --schema

Output format
-------------
The generated module contains a ``TypedTools`` mixin class.  Import it
alongside :class:`~cua_driver.DriverClient` or use the standalone
``TypedDriverClient`` convenience subclass::

    from generated_tools import TypedDriverClient

    with TypedDriverClient() as c:
        apps = c.list_apps()
        pid  = c.launch_app(bundle_id="com.apple.calculator")["structuredContent"]["pid"]
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import textwrap

# Allow running from anywhere in the repo
_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
_SDK_SRC = os.path.join(_THIS_DIR, "..", "python", "src")
sys.path.insert(0, _SDK_SRC)

from cua_driver._client import DriverClient, default_binary_path  # noqa: E402


# ---------------------------------------------------------------------------
# Python identifier helpers
# ---------------------------------------------------------------------------

def _snake(name: str) -> str:
    """``launch_app`` stays ``launch_app``; already snake_case."""
    return name  # cua-driver tools are already snake_case


def _py_type(schema: dict) -> str:
    """Map a JSON Schema type to a Python type annotation string."""
    t = schema.get("type", "")
    if t == "string":
        return "str"
    if t == "integer":
        return "int"
    if t == "number":
        return "float"
    if t == "boolean":
        return "bool"
    if t == "array":
        items = schema.get("items", {})
        return f"list[{_py_type(items)}]"
    if t == "object":
        return "dict"
    return "Any"


def _build_param_list(props: dict, required: list[str]) -> list[tuple[str, str, bool, str]]:
    """Return [(param_name, py_type, is_required, description), ...]."""
    params = []
    for name, spec in props.items():
        py_t = _py_type(spec)
        is_req = name in required
        desc = spec.get("description", "")
        params.append((name, py_t, is_req, desc))
    # required params first
    params.sort(key=lambda p: (not p[2], p[0]))
    return params


# ---------------------------------------------------------------------------
# Code generation
# ---------------------------------------------------------------------------

def _generate_method(tool: dict) -> str:
    """Emit a typed Python method for one MCP tool (unindented, 0-based)."""
    name = tool["name"]
    description = tool.get("description", "")
    input_schema = tool.get("inputSchema", {})
    props = input_schema.get("properties", {})
    required = input_schema.get("required", [])

    params = _build_param_list(props, required)

    # --- Signature ---
    sig_parts = ["self"]
    for pname, pty, is_req, _ in params:
        if is_req:
            sig_parts.append(f"{pname}: {pty}")
        else:
            sig_parts.append(f"{pname}: Optional[{pty}] = None")
    sig = ", ".join(sig_parts)

    # --- Build lines (unindented — indent applied by caller) ---
    lines: list[str] = []
    lines.append(f"def {_snake(name)}({sig}) -> dict:")

    # Docstring: first sanitize description (replace newlines with spaces to
    # keep the generated source syntactically simple).
    safe_desc = " ".join(description.split())
    lines.append(f'    """{safe_desc}')
    if params:
        lines.append("")
        lines.append("    Parameters")
        lines.append("    ----------")
        for pname, pty, is_req, pdesc in params:
            req_marker = " (required)" if is_req else ""
            safe_pdesc = " ".join(pdesc.split()) if pdesc else ""
            lines.append(f"    {pname}: {pty}{req_marker}")
            if safe_pdesc:
                lines.append(f"        {safe_pdesc}")
    lines.append('    """')

    # Body
    lines.append("    arguments: dict = {}")
    for pname, _, is_req, _ in params:
        if is_req:
            lines.append(f'    arguments["{pname}"] = {pname}')
        else:
            lines.append(f'    if {pname} is not None: arguments["{pname}"] = {pname}')
    lines.append(f'    return self.call_tool("{name}", arguments)')

    return "\n".join(lines)


def generate(binary_path: str) -> str:
    """Launch cua-driver, fetch schema, return generated Python source."""
    with DriverClient(binary_path) as client:
        tools = client.list_tools()

    # Indent every method to class body level (4 spaces)
    methods = [textwrap.indent(_generate_method(t), "    ") for t in tools]
    method_block = "\n\n".join(methods)

    tool_names = ", ".join(f'"{t["name"]}"' for t in tools)

    lines = [
        "# AUTO-GENERATED by sdk/codegen/generate.py — do not edit manually.",
        "# Re-run ``python generate.py`` when cua-driver tool definitions change.",
        f'"""Typed wrappers for all {len(tools)} cua-driver MCP tools."""',
        "",
        "from __future__ import annotations",
        "from typing import Any, Optional",
        "from cua_driver import DriverClient",
        "",
        "",
        f"TOOL_NAMES: list[str] = [{tool_names}]",
        "",
        "",
        "class TypedTools:",
        '    """Mixin that adds a typed method for every cua-driver tool.',
        "",
        "    Inherit from this alongside DriverClient::",
        "",
        "        class MyClient(TypedTools, DriverClient):",
        "            pass",
        "",
        "    Or use the pre-built :class:`TypedDriverClient`.",
        '    """',
        "",
        "    # Provided by DriverClient; declared here for type-checker satisfaction.",
        "    def call_tool(self, name: str, arguments: dict | None = None) -> dict: ...",
        "",
        method_block,
        "",
        "",
        "class TypedDriverClient(TypedTools, DriverClient):",
        '    """DriverClient with typed methods for all cua-driver tools."""',
    ]
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate typed Python bindings from live cua-driver MCP schema."
    )
    parser.add_argument(
        "--binary",
        default=default_binary_path(),
        help="Path to the cua-driver binary (default: CUA_DRIVER_BINARY env or debug build)",
    )
    parser.add_argument(
        "--output",
        default="-",
        help="Output file path; '-' writes to stdout (default)",
    )
    parser.add_argument(
        "--schema",
        action="store_true",
        help="Also print the raw tools/list JSON to stderr",
    )
    args = parser.parse_args()

    if args.schema:
        with DriverClient(args.binary) as client:
            tools = client.list_tools()
        print(json.dumps(tools, indent=2), file=sys.stderr)

    source = generate(args.binary)

    if args.output == "-":
        print(source)
    else:
        with open(args.output, "w") as f:
            f.write(source)
        print(f"Written to {args.output}", file=sys.stderr)


if __name__ == "__main__":
    main()

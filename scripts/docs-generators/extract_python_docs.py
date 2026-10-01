#!/usr/bin/env python3
"""Extract Python API documentation with griffe (static: nothing is imported).

Reads a JSON request on stdin and prints JSON on stdout::

    {
      "search_paths": ["libs/python/cua-sandbox"],
      "objects": ["cua_sandbox.image.Image", "cua_sandbox.options.http"],
      "modules": ["cua"]          # optional: module docstring + static tables
    }

Every requested object comes back with its kind, a rendered signature, a
Markdown description (RST literal blocks and ``double backticks`` converted),
the parsed Google-style sections (parameters, returns, raises) and, for
classes, its public members in source order. Output is deterministic: no
paths, dates or host details. The TypeScript side (python-sdk.ts) groups the
objects into pages.

Run with the pinned griffe from requirements.txt::

    uv run --no-project --python 3.12 --with-requirements \\
        scripts/docs-generators/requirements.txt \\
        python scripts/docs-generators/extract_python_docs.py < request.json
"""

from __future__ import annotations

import ast
import json
import re
import sys
from pathlib import Path
from typing import Any

import griffe
from griffe import Alias, Attribute, Class, DocstringSectionKind, Function, Module, ParameterKind

REPO = Path(__file__).resolve().parents[2]

# --------------------------------------------------------------------- text


def rst_to_markdown(text: str) -> str:
    """Converts the RST idioms our docstrings use into Markdown."""
    text = re.sub(r":(?:py:)?(?:class|meth|func|attr|mod|exc|data|obj):`~?([^`]+)`", r"`\1`", text)
    text = re.sub(r"``([^`]+)``", r"`\1`", text)
    lines = text.splitlines()
    out: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.rstrip()
        if stripped.endswith("::") and not stripped.lstrip().startswith(("-", "*", ">>>")):
            lead = stripped[:-2].rstrip()
            base = len(line) - len(line.lstrip())
            j = i + 1
            while j < len(lines) and not lines[j].strip():
                j += 1
            block: list[str] = []
            while j < len(lines) and (
                not lines[j].strip() or len(lines[j]) - len(lines[j].lstrip()) > base
            ):
                block.append(lines[j])
                j += 1
            while block and not block[-1].strip():
                block.pop()
            if block:
                indent = min(len(b) - len(b.lstrip()) for b in block if b.strip())
                if lead:
                    out.append(lead + ":")
                out.append("")
                out.append("```python")
                out.extend(b[indent:] if b.strip() else "" for b in block)
                out.append("```")
                out.append("")
                i = j
                continue
        if stripped.lstrip().startswith(">>>"):
            j = i
            block = []
            while j < len(lines) and lines[j].strip():
                block.append(lines[j].strip())
                j += 1
            out.extend(["", "```python", *block, "```", ""])
            i = j
            continue
        out.append(line)
        i += 1
    joined = "\n".join(out)
    return re.sub(r"\n{3,}", "\n\n", joined).strip()


def expr(value: Any) -> str:
    return "" if value is None else str(value)


# ---------------------------------------------------------------- docstrings


def param_default(obj: Any, name: str) -> str:
    """The Default cell for a documented parameter: ``required`` when the
    signature gives it no default, the literal default otherwise, ``()`` /
    ``{}`` for ``*args`` / ``**kwargs``, '' when the signature does not have it.
    A class's parameters are its ``__init__``'s (or, for a dataclass without
    one, its fields)."""
    name = name.lstrip("*")
    fn = obj
    if isinstance(obj, Class):
        fn = resolve(obj.members.get("__init__")) if "__init__" in obj.members else None
    if isinstance(fn, Function):
        for p in fn.parameters:
            if p.name != name:
                continue
            if p.kind == ParameterKind.var_positional:
                return "()"
            if p.kind == ParameterKind.var_keyword:
                return "{}"
            return "required" if p.default is None else expr(p.default)
    if isinstance(obj, Class):
        field = obj.members.get(name)
        if isinstance(field, Attribute) and "instance-attribute" not in field.labels:
            return "required" if field.value is None else expr(field.value)
    return ""


def sections(obj: Any) -> dict[str, Any]:
    doc: dict[str, Any] = {"description": "", "params": [], "returns": "", "raises": []}
    if not obj.docstring:
        return doc
    text_parts: list[str] = []
    for section in obj.docstring.parse("google"):
        kind = section.kind
        if kind == DocstringSectionKind.text:
            text_parts.append(section.value)
        elif kind == DocstringSectionKind.parameters or kind == DocstringSectionKind.other_parameters:
            for p in section.value:
                doc["params"].append(
                    {
                        "name": p.name,
                        "type": expr(p.annotation),
                        "default": param_default(obj, p.name),
                        "description": rst_to_markdown(p.description or ""),
                    }
                )
        elif kind == DocstringSectionKind.returns:
            doc["returns"] = rst_to_markdown(
                " ".join(r.description or "" for r in section.value).strip()
            )
        elif kind == DocstringSectionKind.raises:
            for r in section.value:
                doc["raises"].append(
                    {"type": expr(r.annotation), "description": rst_to_markdown(r.description or "")}
                )
        elif kind == DocstringSectionKind.examples:
            for sub_kind, value in section.value:
                if str(sub_kind).endswith("examples"):
                    text_parts.append("Example:\n\n```python\n" + value.strip() + "\n```")
                else:
                    text_parts.append(value)
        elif kind == DocstringSectionKind.admonition:
            title = section.title or section.value.kind or ""
            text_parts.append(f"{title.capitalize()}: {section.value.description}")
        else:
            # Unknown sections keep their text so nothing is silently lost.
            value = section.value
            if isinstance(value, str):
                text_parts.append(value)
    doc["description"] = rst_to_markdown("\n\n".join(t.strip() for t in text_parts if t.strip()))
    return doc


# ---------------------------------------------------------------- signatures


def signature(fn: Function, name: str | None = None, *, drop_self: bool = False) -> str:
    params: list[str] = []
    saw_kw_only = saw_pos_only = False
    parameters = list(fn.parameters)
    if drop_self and parameters and parameters[0].name in ("self", "cls"):
        parameters = parameters[1:]
    # Private (underscore) parameters are implementation details.
    parameters = [p for p in parameters if not (p.name.startswith("_") and p.default is not None)]
    for p in parameters:
        if p.kind == ParameterKind.positional_only:
            saw_pos_only = True
        elif saw_pos_only:
            params.append("/")
            saw_pos_only = False
        if p.kind == ParameterKind.keyword_only and not saw_kw_only:
            params.append("*")
            saw_kw_only = True
        text = p.name
        if p.kind == ParameterKind.var_positional:
            text = "*" + text
            saw_kw_only = True
        elif p.kind == ParameterKind.var_keyword:
            text = "**" + text
        if p.annotation is not None:
            text += f": {p.annotation}"
        if p.default is not None and p.kind not in (
            ParameterKind.var_positional,
            ParameterKind.var_keyword,
        ):
            text += f" = {p.default}" if p.annotation is not None else f"={p.default}"
        params.append(text)
    if saw_pos_only:
        params.append("/")
    prefix = "async def " if "async" in fn.labels else "def "
    sig = f"{prefix}{name or fn.name}({', '.join(params)})"
    if fn.returns is not None:
        sig += f" -> {fn.returns}"
    if len(sig) > 88 and params:
        inner = ",\n".join("    " + p for p in params)
        sig = f"{prefix}{name or fn.name}(\n{inner},\n)" + (
            f" -> {fn.returns}" if fn.returns is not None else ""
        )
    return sig


# -------------------------------------------------------------------- objects


def resolve(obj: Any) -> Any:
    seen = 0
    while isinstance(obj, Alias) and seen < 10:
        try:
            obj = obj.target
        except Exception:  # noqa: BLE001 - external or unresolvable target
            return obj
        seen += 1
    return obj


def labels(obj: Any) -> list[str]:
    return sorted(str(label) for label in getattr(obj, "labels", set()))


def function_doc(fn: Function, owner: str | None = None) -> dict[str, Any]:
    is_method = owner is not None
    shown = fn.name
    if fn.name == "__init__" and owner:
        shown = owner.rsplit(".", 1)[-1]
    return {
        "kind": "function",
        "name": fn.name,
        "labels": labels(fn),
        "signature": signature(fn, shown, drop_self=is_method),
        "lineno": fn.lineno or 0,
        **sections(fn),
    }


def attribute_doc(attr: Attribute) -> dict[str, Any]:
    return {
        "kind": "attribute",
        "name": attr.name,
        "labels": labels(attr),
        "type": expr(attr.annotation),
        "value": expr(attr.value) if {"class-attribute", "module-attribute"} & set(attr.labels) else "",
        "lineno": attr.lineno or 0,
        **sections(attr),
    }


_LITERAL_TYPES = {"int": "int", "float": "float", "str": "str", "bool": "bool", "NoneType": ""}


def inferred_type(attr: Attribute, cls: Class) -> str:
    """The type of an unannotated instance attribute, read from what
    ``__init__`` assigns it: a constructor parameter (its annotation), a class
    instantiation (``self.screen = Screen(t)``) or a literal. '' when unknown."""
    value = attr.value
    if value is None:
        return ""
    text = expr(value).split(" or ", 1)[0].strip()
    init = cls.members.get("__init__")
    if isinstance(init, Function):
        for param in init.parameters:
            if param.name == text and param.annotation is not None:
                return expr(param.annotation)
    head = text.split("(", 1)[0]
    if "(" in text and (head.split(".")[-1][:1].isupper() or head in ("list", "dict", "set", "tuple")):
        return head.split(".")[-1]
    try:
        import ast

        literal = ast.literal_eval(text)
    except (ValueError, SyntaxError):
        return ""
    return _LITERAL_TYPES.get(type(literal).__name__, type(literal).__name__)


PUBLIC_DUNDERS = {"__init__", "__call__", "__aenter__", "__aexit__", "__enter__", "__exit__"}


def class_doc(cls: Class) -> dict[str, Any]:
    members: list[dict[str, Any]] = []
    for name, member in cls.members.items():
        member = resolve(member)
        if isinstance(member, Alias):
            continue
        if name.startswith("_") and name not in ("__init__", "__call__"):
            continue
        if isinstance(member, Function):
            if name == "__init__" and not member.parameters:
                continue
            if "property" in member.labels:
                members.append(
                    {
                        "kind": "attribute",
                        "name": name,
                        "labels": labels(member),
                        "type": expr(member.returns),
                        "value": "",
                        "lineno": member.lineno or 0,
                        **sections(member),
                    }
                )
            else:
                members.append(function_doc(member, owner=cls.path))
        elif isinstance(member, Attribute):
            doc = attribute_doc(member)
            if not doc["type"]:
                doc["type"] = inferred_type(member, cls)
            members.append(doc)
        elif isinstance(member, Class):
            continue
    members.sort(key=lambda m: (m["name"] != "__init__", m["lineno"], m["name"]))
    for m in members:
        m.pop("lineno", None)
    return {
        "kind": "class",
        "name": cls.name,
        "labels": labels(cls),
        "bases": [expr(b) for b in cls.bases],
        "members": members,
        **sections(cls),
    }


def object_doc(root: Module, path: str) -> dict[str, Any]:
    rel = path.split(".", 1)[1] if "." in path else ""
    obj = root[rel] if rel else root
    obj = resolve(obj)
    if isinstance(obj, Attribute) and obj.parent is not None and isinstance(obj.value, str) is False:
        # `ImageFileReference = Source`: document the class under the public name.
        target = obj.parent.members.get(expr(obj.value))
        if isinstance(target, Class):
            doc = class_doc(target)
            doc["name"] = obj.name
            doc["path"] = path
            doc["defined_in"] = target.path
            return doc
    if isinstance(obj, Alias):
        return {"path": path, "kind": "external", "name": path.rsplit(".", 1)[-1], "target": obj.target_path}
    if isinstance(obj, Class):
        doc = class_doc(obj)
    elif isinstance(obj, Function):
        doc = function_doc(obj)
    elif isinstance(obj, Attribute):
        doc = attribute_doc(obj)
    elif isinstance(obj, Module):
        doc = {"kind": "module", "name": obj.name, **sections(obj)}
    else:  # pragma: no cover - griffe has no other object kinds
        raise TypeError(f"unsupported object {path}")
    doc.pop("lineno", None)
    doc["path"] = path
    doc["defined_in"] = obj.path
    return doc


# ------------------------------------------------------ module-level tables


def static_tables(module: Module) -> dict[str, Any]:
    """`__all__`, the lazily resolved names (`_LAZY`) and plain rebinding
    aliases (``OldName = _NewName``) of a module, read from its AST."""
    source = Path(module.filepath).read_text()
    tree = ast.parse(source)
    wanted = {"_SANDBOX_NAMES", "_LAZY", "__all__"}
    namespace: dict[str, Any] = {}
    aliases: list[dict[str, str]] = []
    imported_as: dict[str, str] = {}
    imports: dict[str, str] = {}
    for node in tree.body:
        if isinstance(node, ast.ImportFrom):
            for a in node.names:
                if a.asname:
                    imported_as[a.asname] = a.name
                if node.level == 0 and node.module:
                    imports[a.asname or a.name] = node.module
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if isinstance(target, ast.Name) and target.id in wanted:
                code = compile(ast.Module(body=[node], type_ignores=[]), "<static>", "exec")
                exec(code, {"__builtins__": {}}, namespace)  # literals and comprehensions only
            elif (
                isinstance(target, ast.Name)
                and not target.id.startswith("_")
                and isinstance(node.value, ast.Name)
                and node.value.id in imported_as
            ):
                aliases.append({"name": target.id, "target": imported_as[node.value.id]})
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            if node.target.id in wanted and node.value is not None:
                assign = ast.Assign(targets=[node.target], value=node.value)
                ast.copy_location(assign, node)
                code = compile(
                    ast.fix_missing_locations(ast.Module(body=[assign], type_ignores=[])),
                    "<static>",
                    "exec",
                )
                exec(code, {"__builtins__": {}}, namespace)
    lazy = namespace.get("_LAZY", {})
    return {
        "all": list(namespace.get("__all__", [])),
        "lazy": [
            {"name": name, "module": mod, "attribute": attr, "extra": extra}
            for name, (mod, attr, extra) in sorted(lazy.items())
        ],
        "aliases": aliases,
        "imports": dict(sorted(imports.items())),
    }


def module_doc(root: Module, path: str) -> dict[str, Any]:
    rel = path.split(".", 1)[1] if "." in path else ""
    module = root[rel] if rel else root
    doc = {
        "path": path,
        "kind": "module",
        "name": module.name,
        **sections(module),
        **static_tables(module),
    }
    return doc


def main() -> int:
    request = json.load(sys.stdin)
    search = [str((REPO / p).resolve()) for p in request["search_paths"]]
    packages: dict[str, Module] = {}

    def package(path: str) -> Module:
        top = path.split(".", 1)[0]
        if top not in packages:
            packages[top] = griffe.load(
                top,
                search_paths=search,
                docstring_parser="google",
                allow_inspection=False,
                resolve_aliases=False,
            )
        return packages[top]

    objects = [object_doc(package(p), p) for p in request.get("objects", [])]
    modules = [module_doc(package(p), p) for p in request.get("modules", [])]
    json.dump(
        {"griffe": griffe.__name__, "objects": objects, "modules": modules},
        sys.stdout,
        indent=1,
        sort_keys=True,
    )
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

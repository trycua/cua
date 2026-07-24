#!/usr/bin/env python3
"""Generate cua-sandbox reference docs from the imported runtime API.

The generator deliberately uses inspect and typing only. It imports the package in a
minimal environment, starts from cua_sandbox.__all__, and follows public runtime
annotations reachable from those exports.
"""

from __future__ import annotations

import argparse
import importlib
import inspect
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any, Iterable, get_args, get_origin, get_type_hints


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_PACKAGE_ROOT = REPOSITORY_ROOT / "libs/python/cua-sandbox"
DEFAULT_OUTPUT = REPOSITORY_ROOT / "docs/content/docs/reference/sandbox-sdk/index.mdx"
PACKAGE_PREFIX = "cua_sandbox"
BANNED_PARTS = ("cyclops_sdk", "vendored", "vendor", "fleet")
BANNED_MEMBER_PARTS = ("raw", "fleet")


@dataclass(frozen=True)
class Member:
    name: str
    kind: str
    signature: str | None
    description: str
    is_async: bool
    raises: tuple[str, ...]


@dataclass(frozen=True)
class APIItem:
    name: str
    kind: str
    signature: str | None
    description: str
    members: tuple[Member, ...]
    context_manager: str | None
    raises: tuple[str, ...]


@dataclass(frozen=True)
class APIDocument:
    items: tuple[APIItem, ...]


def is_supported_object(value: Any) -> bool:
    module_name = getattr(value, "__module__", "")
    if not module_name.startswith(PACKAGE_PREFIX):
        return False
    parts = module_name.lower().split(".")
    return not any(part in BANNED_PARTS for part in parts)


def is_supported_name(name: str) -> bool:
    lowered = name.lower()
    return not name.startswith("_") and not any(part in lowered for part in BANNED_MEMBER_PARTS)


def is_supported_class(value: type[Any]) -> bool:
    return is_supported_object(value) and is_supported_name(value.__name__)


def is_supported_annotation(annotation: Any) -> bool:
    if annotation is inspect.Signature.empty:
        return True
    if isinstance(annotation, str):
        return not annotation.strip("'\"").startswith("_")
    if inspect.isclass(annotation):
        return is_supported_class(annotation) or not getattr(
            annotation, "__module__", ""
        ).startswith(PACKAGE_PREFIX)
    return True


def callable_signature(value: Any) -> str | None:
    try:
        signature = inspect.signature(value, eval_str=False)
    except (TypeError, ValueError):
        return None
    parameters = [
        parameter.replace(
            annotation=parameter.annotation
            if is_supported_annotation(parameter.annotation)
            else inspect.Signature.empty
        )
        for parameter in signature.parameters.values()
        if is_supported_name(parameter.name)
    ]
    return str(
        signature.replace(
            parameters=parameters,
            return_annotation=signature.return_annotation
            if is_supported_annotation(signature.return_annotation)
            else inspect.Signature.empty,
        )
    )


def doc_parts(value: Any) -> tuple[str, tuple[str, ...]]:
    docstring = inspect.getdoc(value) or ""
    if not docstring:
        return "", ()

    lines = docstring.splitlines()
    description_lines: list[str] = []
    raises: list[str] = []
    in_raises = False
    for line in lines:
        stripped = line.strip()
        if stripped in {"Raises:", "Raise:"}:
            in_raises = True
            continue
        if in_raises:
            if not stripped:
                in_raises = False
                continue
            raises.append(stripped)
            continue
        description_lines.append(line)
    summary_lines: list[str] = []
    for line in description_lines:
        if re.fullmatch(
            r"(?:Args|Arguments|Attributes|Example|Examples|Notes|Returns|Yields):", line.strip()
        ):
            break
        summary_lines.append(line)
    description = "\n".join(summary_lines).strip()
    blocked = BANNED_PARTS + BANNED_MEMBER_PARTS
    paragraphs = [
        paragraph
        for paragraph in description.split("\n\n")
        if not any(part in paragraph.lower() for part in blocked)
    ]
    # Keep rendering thin and MDX-stable: signatures carry the detail, while
    # the first runtime docstring paragraph supplies the user-facing summary.
    description = paragraphs[0].strip() if paragraphs else ""
    description = re.sub(r"``([^`]+)``", r"`\1`", description)
    description = re.sub(r"(?<!\*)\*([^*]+)\*(?!\*)", r"_\1_", description)
    raises = [line for line in raises if not any(part in line.lower() for part in blocked)]
    return description, tuple(raises)


def iter_annotation_classes(annotation: Any) -> Iterable[type[Any]]:
    origin = get_origin(annotation)
    if origin is not None:
        for argument in get_args(annotation):
            yield from iter_annotation_classes(argument)
        return
    if inspect.isclass(annotation) and is_supported_class(annotation):
        yield annotation


def type_hints(value: Any) -> dict[str, Any]:
    try:
        return get_type_hints(value)
    except (NameError, TypeError, ValueError):
        return getattr(value, "__annotations__", {}) or {}


def member_from_descriptor(name: str, descriptor: Any, owner: type[Any]) -> Member | None:
    if not is_supported_name(name):
        return None

    if isinstance(descriptor, property):
        description, raises = doc_parts(descriptor.fget)
        return Member(name, "property", None, description, False, raises)

    value = getattr(owner, name)
    if not (inspect.isroutine(value) or inspect.ismethoddescriptor(value)):
        return None

    description, raises = doc_parts(value)
    return Member(
        name=name,
        kind="method",
        signature=callable_signature(value),
        description=description,
        is_async=inspect.iscoroutinefunction(value) or inspect.isasyncgenfunction(value),
        raises=raises,
    )


def related_classes(value: Any) -> Iterable[type[Any]]:
    for annotation in type_hints(value).values():
        yield from iter_annotation_classes(annotation)


def class_item(value: type[Any]) -> tuple[APIItem, tuple[type[Any], ...]]:
    description, raises = doc_parts(value)
    members: list[Member] = []
    related: list[type[Any]] = list(related_classes(value))

    for name, descriptor in sorted(value.__dict__.items()):
        member = member_from_descriptor(name, descriptor, value)
        if member is not None:
            members.append(member)
        if isinstance(descriptor, property) and descriptor.fget is not None:
            related.extend(related_classes(descriptor.fget))
        elif member is not None:
            related.extend(related_classes(getattr(value, name)))

    context_manager = None
    if "__aenter__" in value.__dict__ and "__aexit__" in value.__dict__:
        context_manager = "Async context manager"
    elif "__enter__" in value.__dict__ and "__exit__" in value.__dict__:
        context_manager = "Context manager"

    item = APIItem(
        name=value.__name__,
        kind="class",
        signature=callable_signature(value),
        description=description,
        members=tuple(members),
        context_manager=context_manager,
        raises=raises,
    )
    return item, tuple(related)


def function_item(name: str, value: Any) -> tuple[APIItem, tuple[type[Any], ...]]:
    description, raises = doc_parts(value)
    item = APIItem(
        name=name,
        kind="function",
        signature=callable_signature(value),
        description=description,
        members=(),
        context_manager=None,
        raises=raises,
    )
    return item, tuple(related_classes(value))


def collect_public_api(module: ModuleType) -> APIDocument:
    """Collect the importable API rooted exclusively at module.__all__."""
    exports = getattr(module, "__all__", None)
    if not isinstance(exports, (list, tuple)):
        raise ValueError("cua_sandbox.__all__ must be a list or tuple")

    items: list[APIItem] = []
    seen_classes: set[type[Any]] = set()
    discovered: set[type[Any]] = set()
    related_queue: list[type[Any]] = []

    def add_value(name: str, value: Any) -> None:
        if not is_supported_object(value):
            return
        if inspect.isclass(value):
            if not is_supported_class(value) or value in seen_classes:
                return
            seen_classes.add(value)
            item, related = class_item(value)
            items.append(item)
            related_queue.extend(related)
            return
        if inspect.isroutine(value):
            item, related = function_item(name, value)
            items.append(item)
            related_queue.extend(related)

    for name in exports:
        if not isinstance(name, str) or name.startswith("_"):
            raise ValueError(f"invalid public export: {name!r}")
        try:
            value = getattr(module, name)
        except AttributeError as error:
            raise ValueError(f"missing public export: {name}") from error
        if (
            not is_supported_name(name)
            or not is_supported_object(value)
            or not (inspect.isclass(value) or inspect.isroutine(value))
        ):
            raise ValueError(f"unsupported public export: {name}")
        add_value(name, value)

    interfaces_module = sys.modules.get(f"{module.__name__}.interfaces")
    if interfaces_module is not None:
        for name in getattr(interfaces_module, "__all__", ()):
            if isinstance(name, str) and is_supported_name(name):
                add_value(name, getattr(interfaces_module, name))

    while related_queue:
        value = related_queue.pop(0)
        if value in seen_classes or value in discovered:
            continue
        discovered.add(value)
        if not is_supported_object(value):
            continue
        add_value(value.__name__, value)

    root_names = [name for name in exports if isinstance(name, str)]
    root_items = [item for item in items if item.name in root_names]
    related_items = [item for item in items if item.name not in root_names]
    related_items.sort(key=lambda item: item.name)
    return APIDocument(tuple(root_items + related_items))


def escape_mdx(text: str) -> str:
    return text.replace("{", "\\{").replace("}", "\\}").replace("<", "&lt;").replace(">", "&gt;")


def render_description(text: str) -> list[str]:
    if not text:
        return []
    return [escape_mdx(line) for line in text.splitlines()] + [""]


def render_raises(raises: tuple[str, ...]) -> list[str]:
    if not raises:
        return []
    lines = ["**Raises**"]
    lines.extend(f"- `{escape_mdx(item)}`" for item in raises)
    return lines + [""]


def render_member(item_name: str, member: Member) -> list[str]:
    lines = [f"### `{item_name}.{member.name}`", ""]
    if member.kind == "property":
        lines.extend(["```python", f"property {member.name}", "```", ""])
    else:
        prefix = "async " if member.is_async else ""
        signature = member.signature or "(...)"
        lines.extend(["```python", f"{prefix}def {member.name}{signature}", "```", ""])
    lines.extend(render_description(member.description))
    lines.extend(render_raises(member.raises))
    return lines


def render_mdx(document: APIDocument) -> str:
    lines = [
        "---",
        "title: Cua Sandbox Python API",
        "description: Runtime-inspected API reference for cua-sandbox",
        "---",
        "",
        "{/* Generated by scripts/docs-generators/cua_sandbox_runtime.py. Do not edit manually. */}",
        "",
        "This reference is generated from the imported `cua_sandbox` runtime, rooted at `cua_sandbox.__all__`.",
        "",
    ]
    for item in document.items:
        lines.extend([f"## `{item.name}`", ""])
        if item.kind == "class":
            signature = item.signature or "(...)"
            lines.extend(["```python", f"class {item.name}{signature}", "```", ""])
        else:
            signature = item.signature or "(...)"
            lines.extend(["```python", f"def {item.name}{signature}", "```", ""])
        if item.context_manager:
            lines.extend([f"**Behavior:** {item.context_manager}.", ""])
        lines.extend(render_description(item.description))
        lines.extend(render_raises(item.raises))
        for member in item.members:
            lines.extend(render_member(item.name, member))
    return "\n".join(lines).rstrip() + "\n"


def check_output(output_path: Path, rendered: str) -> int:
    if not output_path.is_file() or output_path.read_text(encoding="utf-8") != rendered:
        print(f"out of date: {output_path}", file=sys.stderr)
        return 1
    print(f"up to date: {output_path}")
    return 0


class ExcludedCyclopsStub(ModuleType):
    """Permit imports of the explicitly excluded generated Fleet dependency."""

    def __getattr__(self, name: str) -> type[Any]:
        value = type(name, (), {"__module__": "cyclops_sdk"})
        setattr(self, name, value)
        return value


def install_excluded_dependency_stub() -> None:
    if "cyclops_sdk" not in sys.modules:
        sys.modules["cyclops_sdk"] = ExcludedCyclopsStub("cyclops_sdk")
        print("runtime import: stubbed excluded dependency cyclops_sdk", file=sys.stderr)


def controlled_import_environment() -> None:
    """Remove caller configuration that could alter import-time package behavior."""
    preserved = {
        key: os.environ[key] for key in ("PATH", "SYSTEMROOT", "WINDIR") if key in os.environ
    }
    os.environ.clear()
    os.environ.update(preserved)
    os.environ["PYTHONNOUSERSITE"] = "1"
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    os.environ["CUA_SANDBOX_DOCS_RUNTIME_INSPECTION"] = "1"


def import_cua_sandbox(package_root: Path) -> ModuleType:
    package_root = package_root.resolve()
    if not (package_root / PACKAGE_PREFIX / "__init__.py").is_file():
        raise ValueError(f"not a cua-sandbox package root: {package_root}")
    sys.path.insert(0, str(package_root))
    importlib.invalidate_caches()
    return importlib.import_module(PACKAGE_PREFIX)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if generated output differs")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--package-root", type=Path, default=DEFAULT_PACKAGE_ROOT)
    parser.add_argument("--no-excluded-dependency-stub", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    controlled_import_environment()
    if not args.no_excluded_dependency_stub:
        install_excluded_dependency_stub()
    module = import_cua_sandbox(args.package_root)
    rendered = render_mdx(collect_public_api(module))
    if args.check:
        return check_output(args.output, rendered)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(rendered, encoding="utf-8", newline="\n")
    print(f"wrote: {args.output}")
    print(f"bytes: {len(rendered.encode('utf-8'))}; lines: {rendered.count(chr(10))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

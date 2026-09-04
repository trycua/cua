#!/usr/bin/env python3
"""Normalize third-party Go and Node UniFFI output to the compatibility ABI."""

from __future__ import annotations

import argparse
import re
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
GO_OUTPUT = ROOT / "cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk_schema/cyclops_sdk_schema.go"
NODE_OUTPUT = ROOT / "cyclops-cs/sdk-bindings/ts-uniffi/cyclops_sdk_schema.ts"
BUILDER_NAME = "Builder"
GO_BUILDER_TYPE = re.compile(
    r"^type (?P<name>[A-Za-z_][A-Za-z0-9_]*Builder) (?:struct|interface)",
    re.MULTILINE,
)
GO_DECLARATION = re.compile(r"(?=^(?:type|func|var|const) )", re.MULTILINE)
NODE_DECLARATION = re.compile(
    r"^(?:export\s+)?(?P<kind>interface|type|class|enum|const|function)\s+"
    r"(?P<name>[A-Za-z_$][A-Za-z0-9_$]*)",
    re.MULTILINE,
)


def require_all(source: str, snippets: list[str], label: str) -> None:
    missing = [snippet for snippet in snippets if snippet not in source]
    if missing:
        raise ValueError(f"raw {label} generator output is missing: {missing[0]!r}")


def normalize_trailing_whitespace(source: str) -> str:
    lines = [line.rstrip() for line in source.splitlines()]
    while lines and not lines[-1]:
        lines.pop()
    return "\n".join(lines) + "\n"


def matching_brace(source: str, opening_brace: int) -> int:
    """Return the matching closing brace while skipping Go string and comment text."""
    depth = 0
    index = opening_brace
    state = "code"
    while index < len(source):
        character = source[index]
        following = source[index + 1] if index + 1 < len(source) else ""
        if state == "code":
            if character == '"':
                state = "string"
            elif character == "'":
                state = "rune"
            elif character == "`":
                state = "raw_string"
            elif character == "/" and following == "/":
                state = "line_comment"
                index += 1
            elif character == "/" and following == "*":
                state = "block_comment"
                index += 1
            elif character == "{":
                depth += 1
            elif character == "}":
                depth -= 1
                if depth == 0:
                    return index
        elif state in {"string", "rune"}:
            if character == "\\":
                index += 1
            elif (state == "string" and character == '"') or (
                state == "rune" and character == "'"
            ):
                state = "code"
        elif state == "raw_string" and character == "`":
            state = "code"
        elif state == "line_comment" and character == "\n":
            state = "code"
        elif state == "block_comment" and character == "*" and following == "/":
            state = "code"
            index += 1
        index += 1
    raise ValueError("raw Go generator output has an unmatched brace")


def go_builder_classes(source: str) -> set[str]:
    return {match.group("name") for match in GO_BUILDER_TYPE.finditer(source)}


def remove_go_builder_checksums(source: str, builder_classes: set[str]) -> str:
    marker = "func uniffiCheckChecksums() {"
    start = source.index(marker)
    opening_brace = source.index("{", start)
    closing_brace = matching_brace(source, opening_brace)
    checksums = source[opening_brace + 1 : closing_brace]
    builder_symbols = {builder_class.lower() for builder_class in builder_classes}
    normalized: list[str] = []
    last = 0
    for match in re.finditer(r"^\t\{", checksums, re.MULTILINE):
        block_end = matching_brace(checksums, match.start()) + 1
        block = checksums[match.start() : block_end]
        if any(f"{symbol}_" in block for symbol in builder_symbols):
            normalized.append(checksums[last : match.start()])
            last = block_end
    normalized.append(checksums[last:])
    return source[: opening_brace + 1] + "".join(normalized) + source[closing_brace:]


def go_builder_identifiers(builder_classes: set[str]) -> set[str]:
    identifiers: set[str] = set()
    for builder_class in builder_classes:
        identifiers.update(
            {
                builder_class,
                f"{builder_class}Interface",
                f"FfiConverter{builder_class}",
                f"FfiConverter{builder_class}INSTANCE",
                f"FfiDestroyer{builder_class}",
                f"New{builder_class}",
                f"LiftFromExternal{builder_class}",
                f"LowerToExternal{builder_class}",
            }
        )
    return identifiers


def remove_go_builder_declarations(source: str, builder_classes: set[str]) -> str:
    declarations = GO_DECLARATION.split(source)
    builder_identifiers = go_builder_identifiers(builder_classes)
    source = "".join(
        declaration
        for declaration in declarations
        if not builder_identifiers.intersection(
            re.findall(r"[A-Za-z_][A-Za-z0-9_]*", declaration)
        )
    )
    if "runtime." not in source:
        source = source.replace('\n\t"runtime"\n', "\n")
    return source


def format_go(source: str) -> str:
    formatted = subprocess.run(
        ["gofmt"], input=source, text=True, capture_output=True, check=False
    )
    if formatted.returncode:
        raise ValueError(f"could not gofmt compatibility Go output: {formatted.stderr}")
    return formatted.stdout


def normalize_go(raw: str) -> str:
    require_all(
        raw,
        [
            "TtlSecondsAfterCreated *uint32",
            "FfiConverterOptionalUint32INSTANCE.Write(writer, value.TtlSecondsAfterCreated)",
        ],
        "Go",
    )
    builder_classes = go_builder_classes(raw)
    if not builder_classes:
        raise ValueError("raw Go generator output is missing builder declarations")
    return format_go(
        normalize_trailing_whitespace(
            remove_go_builder_declarations(
                remove_go_builder_checksums(raw, builder_classes), builder_classes
            )
        )
    )


def node_builder_classes(declarations: list[re.Match[str]]) -> set[str]:
    return {
        declaration.group("name")
        for declaration in declarations
        if declaration.group("kind") == "class"
        and declaration.group("name").endswith(BUILDER_NAME)
    }


def node_builder_declaration_names(
    declarations: list[re.Match[str]], builder_classes: set[str]
) -> set[str]:
    names = {
        declaration.group("name")
        for declaration in declarations
        if declaration.group("name").endswith(BUILDER_NAME)
    }
    for builder_class in builder_classes:
        names.update(
            {
                f"{builder_class}Like",
                f"{builder_class}Interface",
                f"uniffiType{builder_class}ObjectFactory",
                f"FfiConverterType{builder_class}",
            }
        )
    return names


def remove_node_builder_checksums(source: str, builder_classes: set[str]) -> str:
    builder_symbols = {builder_class.lower() for builder_class in builder_classes}

    def replace(match: re.Match[str]) -> str:
        return "" if any(f"{symbol}_" in match.group() for symbol in builder_symbols) else match.group()

    return re.sub(
        r"^    if \(nativeModule\(\)\.[^\n]*\n.*?^    \}\n",
        replace,
        source,
        flags=re.MULTILINE | re.DOTALL,
    )


def remove_node_builder_default_exports(source: str, builder_names: set[str]) -> str:
    marker = "export default Object.freeze({"
    before, separator, exports = source.partition(marker)
    if not separator:
        raise ValueError("raw Node generator output is missing default converter exports")
    builder_converters = {
        builder_name
        for builder_name in builder_names
        if builder_name.startswith("FfiConverterType")
    }
    return before + separator + "".join(
        line
        for line in exports.splitlines(keepends=True)
        if line.strip().rstrip(",") not in builder_converters
    )


def remove_node_builder_declarations(
    source: str, declarations: list[re.Match[str]], builder_names: set[str]
) -> str:
    normalized: list[str] = [source[: declarations[0].start()]]
    for index, declaration in enumerate(declarations):
        end = declarations[index + 1].start() if index + 1 < len(declarations) else len(source)
        if declaration.group("name") not in builder_names:
            normalized.append(source[declaration.start() : end])
    return "".join(normalized)


def normalize_node(raw: str) -> str:
    require_all(
        raw,
        [
            "ttlSecondsAfterCreated?: number",
            "ttlSecondsAfterCreated: FfiConverterOptionalUInt32.read(from)",
            "FfiConverterOptionalUInt32.write(value.ttlSecondsAfterCreated, into)",
        ],
        "Node",
    )
    declarations = list(NODE_DECLARATION.finditer(raw))
    if not declarations:
        raise ValueError("raw Node generator output is missing top-level declarations")
    builder_classes = node_builder_classes(declarations)
    builder_names = node_builder_declaration_names(declarations, builder_classes)
    source = remove_node_builder_default_exports(
        remove_node_builder_checksums(raw, builder_classes), builder_names
    )
    return normalize_trailing_whitespace(
        remove_node_builder_declarations(
            source, list(NODE_DECLARATION.finditer(source)), builder_names
        )
    )


def check_no_builders(source: str, path: Path) -> None:
    if path == NODE_OUTPUT:
        for declaration in NODE_DECLARATION.finditer(source):
            if declaration.group("name").endswith(BUILDER_NAME):
                raise ValueError(f"compatibility binding exposes builder ABI: {path}")
        return
    if GO_BUILDER_TYPE.search(source):
        raise ValueError(f"compatibility binding exposes builder ABI: {path}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw-go", type=Path, required=True)
    parser.add_argument("--raw-node", type=Path, required=True)
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()

    outputs = (
        (GO_OUTPUT, normalize_go, arguments.raw_go, "Go"),
        (NODE_OUTPUT, normalize_node, arguments.raw_node, "Node"),
    )
    for output, normalize, raw_path, label in outputs:
        normalized = normalize(raw_path.read_text())
        check_no_builders(normalized, output)
        if arguments.check:
            if output.read_text() != normalized:
                raise SystemExit(f"normalized {label} binding drift detected: {output}")
        else:
            output.write_text(normalized)


if __name__ == "__main__":
    main()

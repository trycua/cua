import argparse
import json
import re
import subprocess
from pathlib import Path

import tree_sitter_rust
from tree_sitter import Language, Parser


parser = Parser(Language(tree_sitter_rust.language()))


def production_source(path, normalize=False):
    source = path.read_bytes()
    if normalize:
        source = subprocess.run(
            ["rustfmt", "+1.97.1", "--edition", "2021", "--emit", "stdout", "--config", "skip_children=true"],
            input=source, capture_output=True, check=True,
        ).stdout
    tree = parser.parse(source)
    excluded = []
    external = []

    def visit(node):
        attributes = []
        for child in node.named_children:
            if child.type == "attribute_item":
                attributes.append(child)
                continue
            if child.type in ("line_comment", "block_comment"):
                continue
            text = b" ".join(source[a.start_byte:a.end_byte] for a in attributes)
            test_only = re.search(rb"cfg\s*\(\s*(?:test\s*\)|all\(\s*test\s*[,\)])|#\[\s*(?:tokio::)?test(?:\]|\()", text)
            if test_only:
                excluded.append((attributes[0].start_byte, child.end_byte))
                if child.type == "mod_item" and child.child_by_field_name("body") is None:
                    explicit = re.search(rb'path\s*=\s*"([^"]+)"', text)
                    name = child.child_by_field_name("name")
                    if explicit:
                        external.append(path.parent / explicit.group(1).decode())
                    elif name:
                        module = source[name.start_byte:name.end_byte].decode()
                        external.extend((path.parent / f"{module}.rs", path.parent / module))
            else:
                visit(child)
            attributes = []

    visit(tree.root_node)
    output = bytearray(source)
    for start, end in excluded:
        output[start:end] = bytes(10 if byte == 10 else 32 for byte in source[start:end])
    return output.decode(), external


def code_and_complexity(source):
    data = source.encode()
    tree = parser.parse(data)
    output = bytearray(data)
    functions = []

    def decisions(node):
        if node.type == "function_item":
            return 0
        count = int(node.type in ("if_expression", "for_expression", "while_expression", "loop_expression", "try_expression"))
        if node.type == "let_declaration":
            count += int(node.child_by_field_name("alternative") is not None)
        if node.type == "binary_expression":
            operator = node.child_by_field_name("operator")
            count += int(operator is not None and data[operator.start_byte:operator.end_byte] in (b"&&", b"||"))
        if node.type == "match_expression":
            body = node.child_by_field_name("body")
            count += max(0, sum(child.type == "match_arm" for child in body.named_children) - 1)
        if node.type == "match_pattern":
            count += int(node.child_by_field_name("condition") is not None)
        return count + sum(decisions(child) for child in node.named_children)

    def visit(node):
        if node.type in ("line_comment", "block_comment"):
            output[node.start_byte:node.end_byte] = bytes(10 if byte == 10 else 32 for byte in data[node.start_byte:node.end_byte])
            return
        if node.type == "function_item":
            body = node.child_by_field_name("body")
            if body is not None:
                functions.append({
                    "name": " ".join(data[node.start_byte:body.start_byte].decode().split()),
                    "line": node.start_point.row + 1,
                    "ccn": 1 + decisions(body),
                })
        for child in node.named_children:
            visit(child)

    visit(tree.root_node)
    lines = [line.rstrip() for line in output.decode().splitlines() if line.strip()]
    return lines, functions, tree.root_node.has_error


def measure(root):
    crates = root / "libs/cua-driver/rust/crates"
    packages = ("cua-driver-core", "cua-driver-sdk", "platform-macos", "platform-windows", "platform-linux")
    paths = sorted(path for package in packages for path in (crates / package / "src").rglob("*.rs"))
    sources = {}
    excluded = set()
    for path in paths:
        source, external = production_source(path, normalize=True)
        sources[path] = source
        excluded.update(external)
    result = {}
    for path, source in sources.items():
        if any(path == ignored or ignored in path.parents for ignored in excluded):
            continue
        lines, functions, parse_error = code_and_complexity(source)
        result[path.relative_to(root).as_posix()] = {
            "functions": functions,
            "ccn": sum(function["ccn"] for function in functions),
            "decision_surplus": sum(function["ccn"] - 1 for function in functions),
            "nloc": len(lines),
            "parse_error": parse_error,
        }
    return {
        "method": "rustfmt +1.97.1; tree-sitter 0.25.2 / tree-sitter-rust 0.24.2; test-only items/modules and comments excluded. Per function: 1 + if/let-else/for/while/loop/? + &&/|| + match arms minus 1 + match guards. Closure decisions belong to their enclosing function; nested functions counted separately; macros are not expanded.",
        "totals": {key: sum(file[key] for file in result.values()) for key in ("ccn", "decision_surplus", "nloc")},
        "files": result,
    }


if __name__ == "__main__":
    arguments = argparse.ArgumentParser()
    arguments.add_argument("root", type=Path)
    options = arguments.parse_args()
    print(json.dumps(measure(options.root.resolve()), indent=2))

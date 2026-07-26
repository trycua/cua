"""Compatibility lock for the released 0.12.6 Python package surface."""

from __future__ import annotations

import ast
import json
from pathlib import Path


PACKAGE_ROOT = Path(__file__).parents[1]
FIXTURE = (
    PACKAGE_ROOT.parent / "compat-fixtures" / "python-package.json"
)


def _module(path: Path) -> ast.Module:
    return ast.parse(path.read_text(encoding="utf-8"))


def _signature(node: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    arguments = node.args
    positional = [*arguments.posonlyargs, *arguments.args]
    defaults = [None] * (len(positional) - len(arguments.defaults)) + list(
        arguments.defaults
    )
    rendered = []
    for argument, default in zip(positional, defaults):
        value = argument.arg
        if argument.annotation is not None:
            value += f": {ast.unparse(argument.annotation)}"
        if default is not None:
            value += f" = {ast.unparse(default)}"
        rendered.append(value)

    if arguments.vararg is not None:
        rendered.append(f"*{arguments.vararg.arg}")
    elif arguments.kwonlyargs:
        rendered.append("*")
    for argument, default in zip(arguments.kwonlyargs, arguments.kw_defaults):
        value = argument.arg
        if argument.annotation is not None:
            value += f": {ast.unparse(argument.annotation)}"
        if default is not None:
            value += f" = {ast.unparse(default)}"
        rendered.append(value)
    if arguments.kwarg is not None:
        rendered.append(f"**{arguments.kwarg.arg}")

    prefix = "async " if isinstance(node, ast.AsyncFunctionDef) else ""
    result = f"{prefix}{node.name}({', '.join(rendered)})"
    if node.returns is not None:
        result += f" -> {ast.unparse(node.returns)}"
    return result


def _functions(module: ast.Module) -> dict[str, ast.FunctionDef | ast.AsyncFunctionDef]:
    return {
        node.name: node
        for node in module.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }


def test_released_python_exports_and_signatures_remain_available() -> None:
    expected = json.loads(FIXTURE.read_text(encoding="utf-8"))
    package_module = _module(PACKAGE_ROOT / "src" / "cua_driver" / "__init__.py")
    wrapper_module = _module(PACKAGE_ROOT / "src" / "cua_driver" / "wrapper.py")
    native_module = _module(PACKAGE_ROOT / "src" / "cua_driver" / "_native.py")

    all_assignment = next(
        node
        for node in package_module.body
        if isinstance(node, ast.Assign)
        and any(
            isinstance(target, ast.Name) and target.id == "__all__"
            for target in node.targets
        )
    )
    actual_exports = {
        element.value
        for element in all_assignment.value.elts
        if isinstance(element, ast.Constant) and isinstance(element.value, str)
    }
    assert set(expected["package_root_exports"]) <= actual_exports

    package_functions = _functions(package_module)
    package_signatures = {
        _signature(package_functions["_connect_python_sdk"]),
        _signature(package_functions["_create_python_sdk"]),
    }
    assert set(expected["package_constructor_signatures"]) == package_signatures

    wrapper_functions = _functions(wrapper_module)
    wrapper_signatures = {
        _signature(wrapper_functions["get_binary_path"]),
        _signature(wrapper_functions["run_cua_driver"]),
    }
    assert set(expected["wrapper_signatures"]) == wrapper_signatures

    driver_class = next(
        node
        for node in native_module.body
        if isinstance(node, ast.ClassDef) and node.name == "CuaDriver"
    )
    actual_methods = {
        _signature(node)
        for node in driver_class.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        and not node.name.startswith("_")
    }
    assert set(expected["cua_driver_methods"]) <= actual_methods

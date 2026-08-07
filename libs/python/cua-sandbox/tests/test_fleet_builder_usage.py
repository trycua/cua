from __future__ import annotations

import ast
from pathlib import Path


PACKAGE_ROOT = Path(__file__).parents[1]
FLEET_MODULES = {"cua_sandbox", "fleet_sdk"}
BUILDER_ENABLED_RECORDS = {
    "CreatePoolRequest",
    "CreateTemplateRequest",
    "OsGymSandboxTemplateSpec",
    "OsGymSandboxWarmPoolSpec",
    "SandboxService",
    "SandboxTemplateRef",
    "VmTemplate",
}
MODULE_ALIAS = object()
OTHER_BINDING = object()


class _BindingScope:
    def __init__(self, parent: _BindingScope | None, kind: str) -> None:
        self.parent = parent
        self.kind = kind
        self.bindings: dict[str, object] = {}

    def bind(self, name: str, binding: object) -> None:
        self.bindings[name] = binding

    def resolve(self, name: str) -> object | None:
        scope: _BindingScope | None = self
        while scope is not None:
            if name in scope.bindings:
                return scope.bindings[name]
            scope = scope.parent
        return None


class _BuilderRecordCallVisitor(ast.NodeVisitor):
    def __init__(self) -> None:
        self.calls: list[tuple[int, str]] = []
        self.scope = _BindingScope(parent=None, kind="module")

    def _bind_other(self, name: str) -> None:
        self.scope.bind(name, OTHER_BINDING)

    def _bind_target(self, target: ast.expr) -> None:
        if isinstance(target, ast.Name):
            self._bind_other(target.id)
        elif isinstance(target, (ast.List, ast.Tuple)):
            for element in target.elts:
                self._bind_target(element)
        elif isinstance(target, ast.Starred):
            self._bind_target(target.value)

    def _visit_arguments(self, arguments: ast.arguments) -> None:
        positional = (*arguments.posonlyargs, *arguments.args)
        for argument in (*positional, *arguments.kwonlyargs):
            if argument.annotation is not None:
                self.visit(argument.annotation)
        if arguments.vararg is not None and arguments.vararg.annotation is not None:
            self.visit(arguments.vararg.annotation)
        if arguments.kwarg is not None and arguments.kwarg.annotation is not None:
            self.visit(arguments.kwarg.annotation)
        for default in (*arguments.defaults, *arguments.kw_defaults):
            if default is not None:
                self.visit(default)

    def _bind_arguments(self, arguments: ast.arguments) -> None:
        for argument in (*arguments.posonlyargs, *arguments.args, *arguments.kwonlyargs):
            self._bind_other(argument.arg)
        if arguments.vararg is not None:
            self._bind_other(arguments.vararg.arg)
        if arguments.kwarg is not None:
            self._bind_other(arguments.kwarg.arg)

    def _visit_function(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        for decorator in node.decorator_list:
            self.visit(decorator)
        self._visit_arguments(node.args)
        if node.returns is not None:
            self.visit(node.returns)
        for type_parameter in getattr(node, "type_params", ()):
            self.visit(type_parameter)

        outer_scope = self.scope
        self._bind_other(node.name)
        parent_scope = outer_scope.parent if outer_scope.kind == "class" else outer_scope
        self.scope = _BindingScope(parent=parent_scope, kind="function")
        try:
            self._bind_arguments(node.args)
            for statement in node.body:
                self.visit(statement)
        finally:
            self.scope = outer_scope

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        for alias in node.names:
            if alias.name == "*":
                continue
            local_name = alias.asname or alias.name
            if node.level == 0 and node.module in FLEET_MODULES and alias.name in BUILDER_ENABLED_RECORDS:
                self.scope.bind(local_name, alias.name)
            else:
                self._bind_other(local_name)

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            local_name = alias.asname or alias.name.split(".", maxsplit=1)[0]
            binding = MODULE_ALIAS if alias.name in FLEET_MODULES else OTHER_BINDING
            self.scope.bind(local_name, binding)

    def visit_Call(self, node: ast.Call) -> None:
        record_name: str | None = None
        if isinstance(node.func, ast.Name):
            binding = self.scope.resolve(node.func.id)
            if isinstance(binding, str):
                record_name = binding
        elif (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and self.scope.resolve(node.func.value.id) is MODULE_ALIAS
            and node.func.attr in BUILDER_ENABLED_RECORDS
        ):
            record_name = node.func.attr

        if record_name is not None:
            self.calls.append((node.lineno, record_name))
        self.generic_visit(node)

    def visit_Assign(self, node: ast.Assign) -> None:
        self.visit(node.value)
        for target in node.targets:
            self._bind_target(target)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        self.visit(node.annotation)
        if node.value is not None:
            self.visit(node.value)
        self._bind_target(node.target)

    def visit_AugAssign(self, node: ast.AugAssign) -> None:
        self.visit(node.target)
        self.visit(node.value)
        self._bind_target(node.target)

    def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
        self.visit(node.value)
        self._bind_target(node.target)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_function(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_function(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for decorator in node.decorator_list:
            self.visit(decorator)
        for base in node.bases:
            self.visit(base)
        for keyword in node.keywords:
            self.visit(keyword)
        for type_parameter in getattr(node, "type_params", ()):
            self.visit(type_parameter)

        outer_scope = self.scope
        self.scope = _BindingScope(parent=outer_scope, kind="class")
        try:
            for statement in node.body:
                self.visit(statement)
        finally:
            self.scope = outer_scope
        self._bind_other(node.name)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self._visit_arguments(node.args)
        outer_scope = self.scope
        parent_scope = outer_scope.parent if outer_scope.kind == "class" else outer_scope
        self.scope = _BindingScope(parent=parent_scope, kind="function")
        try:
            self._bind_arguments(node.args)
            self.visit(node.body)
        finally:
            self.scope = outer_scope


def _find_builder_record_calls(source: str, *, filename: str = "<unknown>") -> list[tuple[int, str]]:
    visitor = _BuilderRecordCallVisitor()
    visitor.visit(ast.parse(source, filename=filename))
    return visitor.calls


def test_finds_direct_builder_record_imports() -> None:
    source = (
        "from fleet_sdk import VmTemplate\n"
        "from cua_sandbox import SandboxService\n"
        "VmTemplate()\n"
        "SandboxService()\n"
    )

    assert _find_builder_record_calls(source) == [(3, "VmTemplate"), (4, "SandboxService")]


def test_finds_aliased_builder_record_imports() -> None:
    source = (
        "from fleet_sdk import VmTemplate as FleetVm\n"
        "from cua_sandbox import SandboxService as FleetService\n"
        "FleetVm()\n"
        "FleetService()\n"
    )

    assert _find_builder_record_calls(source) == [(3, "VmTemplate"), (4, "SandboxService")]


def test_finds_builder_record_module_attributes() -> None:
    source = (
        "import fleet_sdk as fleet\n"
        "import cua_sandbox as cua\n"
        "fleet.VmTemplate()\n"
        "cua.SandboxService()\n"
    )

    assert _find_builder_record_calls(source) == [(3, "VmTemplate"), (4, "SandboxService")]


def test_ignores_unrelated_local_builder_record_name() -> None:
    source = "class VmTemplate:\n    pass\n\nVmTemplate()\n"

    assert _find_builder_record_calls(source) == []


def test_ignores_direct_import_after_local_shadowing() -> None:
    sources = (
        "from fleet_sdk import VmTemplate\nclass VmTemplate:\n    pass\nVmTemplate()\n",
        "from fleet_sdk import VmTemplate\ndef VmTemplate():\n    pass\nVmTemplate()\n",
        "from fleet_sdk import VmTemplate\nVmTemplate = object()\nVmTemplate()\n",
    )

    for source in sources:
        assert _find_builder_record_calls(source) == []


def test_function_imports_do_not_leak_to_sibling_or_outer_scopes() -> None:
    source = (
        "def fleet_user():\n"
        "    from fleet_sdk import VmTemplate\n"
        "    VmTemplate()\n"
        "\n"
        "def sibling():\n"
        "    class VmTemplate:\n"
        "        pass\n"
        "    VmTemplate()\n"
        "\n"
        "def VmTemplate():\n"
        "    pass\n"
        "\n"
        "VmTemplate()\n"
    )

    assert _find_builder_record_calls(source) == [(3, "VmTemplate")]


def test_ignores_module_attribute_after_alias_rebinding() -> None:
    source = "import fleet_sdk as fleet\nfleet = object()\nfleet.VmTemplate()\n"

    assert _find_builder_record_calls(source) == []


def test_nested_scope_resolves_unshadowed_outer_fleet_binding() -> None:
    source = "from fleet_sdk import VmTemplate\ndef build():\n    VmTemplate()\n"

    assert _find_builder_record_calls(source) == [(3, "VmTemplate")]


def test_builder_enabled_fleet_records_use_generated_builders() -> None:
    violations: list[str] = []
    source_roots = (PACKAGE_ROOT / "cua_sandbox", PACKAGE_ROOT / "tests")

    for source_root in source_roots:
        for path in source_root.rglob("*.py"):
            calls = _find_builder_record_calls(path.read_text(), filename=str(path))
            relative_path = path.relative_to(PACKAGE_ROOT)
            violations.extend(f"{relative_path}:{line}: {name}" for line, name in calls)

    assert violations == [], "Direct Fleet record constructors remain:\n" + "\n".join(violations)

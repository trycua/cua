"""
Helper functions and decorators for the Computer module.
"""

import ast
import asyncio
import builtins
import importlib.util
import inspect
import logging
import os
import sys
from functools import wraps
from inspect import getsource
from textwrap import dedent
from types import FunctionType, ModuleType
from typing import Any, Awaitable, Callable, Dict, List, Set, TypedDict, TypeVar

try:
    # Python 3.12+ has ParamSpec in typing
    from typing import ParamSpec
except ImportError:  # pragma: no cover
    # Fallback for environments without ParamSpec in typing
    from typing_extensions import ParamSpec  # type: ignore

P = ParamSpec("P")
R = TypeVar("R")


class DependencyInfo(TypedDict):
    import_statements: List[str]
    definitions: List[tuple[str, Any]]


# Global reference to the default computer instance
_default_computer = None

# Global cache for function dependency analysis
_function_dependency_map: Dict[FunctionType, DependencyInfo] = {}

logger = logging.getLogger(__name__)


def set_default_computer(computer: Any) -> None:
    """
    Set the default computer instance to be used by the remote decorator.

    Args:
        computer: The computer instance to use as default
    """
    global _default_computer
    _default_computer = computer


def sandboxed(
    venv_name: str = "default",
    computer: str = "default",
    max_retries: int = 3,
) -> Callable[[Callable[P, R]], Callable[P, Awaitable[R]]]:
    """
    Decorator that wraps a function to be executed remotely via computer.venv_exec

    The function is automatically analyzed for dependencies (imports, helper functions,
    constants, etc.) and reconstructed with all necessary code in the remote sandbox.

    Args:
        venv_name: Name of the virtual environment to execute in
        computer: The computer instance to use, or "default" to use the globally set default
        max_retries: Maximum number of retries for the remote execution
    """

    def decorator(func: Callable[P, R]) -> Callable[P, Awaitable[R]]:
        @wraps(func)
        async def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
            # Determine which computer instance to use
            comp = computer if computer != "default" else _default_computer

            if comp is None:
                raise RuntimeError(
                    "No computer instance available. Either specify a computer instance or call set_default_computer() first."
                )

            for i in range(max_retries):
                try:
                    return await comp.venv_exec(venv_name, func, *args, **kwargs)
                except Exception as e:
                    logger.error(f"Attempt {i+1} failed: {e}")
                    await asyncio.sleep(1)
                    if i == max_retries - 1:
                        raise e

            # Should be unreachable because we either returned or raised
            raise RuntimeError("sandboxed wrapper reached unreachable code path")

        return wrapper

    return decorator


def _extract_import_statement(name: str, module: ModuleType) -> str:
    """Extract the original import statement for a module."""
    module_name = module.__name__

    if name == module_name.split(".")[0]:
        return f"import {module_name}"
    else:
        return f"import {module_name} as {name}"


def _is_third_party_module(module_name: str) -> bool:
    """Check if a module is a third-party module."""
    stdlib_modules = set(sys.stdlib_module_names) if hasattr(sys, "stdlib_module_names") else set()

    if module_name in stdlib_modules:
        return False

    try:
        spec = importlib.util.find_spec(module_name)
        if spec is None:
            return False

        if spec.origin and ("site-packages" in spec.origin or "dist-packages" in spec.origin):
            return True

        return False
    except (ImportError, ModuleNotFoundError, ValueError):
        return False


def _is_project_import(module_name: str) -> bool:
    """Check if a module is a project-level import."""
    if module_name.startswith("__relative_import_level_"):
        return True

    if module_name in sys.modules:
        module = sys.modules[module_name]
        if hasattr(module, "__file__") and module.__file__:
            if "site-packages" not in module.__file__ and "dist-packages" not in module.__file__:
                cwd = os.getcwd()
                if module.__file__.startswith(cwd):
                    return True

    return False


def _categorize_module(module_name: str) -> str:
    """Categorize a module as stdlib, third-party, or project."""
    if module_name.startswith("__relative_import_level_"):
        return "project"
    elif module_name in (
        set(sys.stdlib_module_names) if hasattr(sys, "stdlib_module_names") else set()
    ):
        return "stdlib"
    elif _is_third_party_module(module_name):
        return "third_party"
    elif _is_project_import(module_name):
        return "project"
    else:
        return "unknown"


class _DependencyVisitor(ast.NodeVisitor):
    """AST visitor to extract imports and name references from a function."""

    def __init__(self, function_name: str) -> None:
        self.function_name = function_name
        self.internal_imports: Set[str] = set()
        self.internal_import_statements: List[str] = []
        self.name_references: Set[str] = set()
        self.local_names: Set[str] = set()
        self.inside_function = False

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        if node.name == self.function_name and not self.inside_function:
            self.inside_function = True

            for arg in node.args.args + node.args.posonlyargs + node.args.kwonlyargs:
                self.local_names.add(arg.arg)
            if node.args.vararg:
                self.local_names.add(node.args.vararg.arg)
            if node.args.kwarg:
                self.local_names.add(node.args.kwarg.arg)

            for child in node.body:
                self.visit(child)

            self.inside_function = False
        else:
            if self.inside_function:
                self.local_names.add(node.name)
                for child in node.body:
                    self.visit(child)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.visit_FunctionDef(node)  # type: ignore

    def visit_Import(self, node: ast.Import) -> None:
        if self.inside_function:
            for alias in node.names:
                module_name = alias.name.split(".")[0]
                self.internal_imports.add(module_name)
                imported_as = alias.asname if alias.asname else alias.name.split(".")[0]
                self.local_names.add(imported_as)
            self.internal_import_statements.append(ast.unparse(node))
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if self.inside_function:
            if node.level == 0 and node.module:
                module_name = node.module.split(".")[0]
                self.internal_imports.add(module_name)
            elif node.level > 0:
                self.internal_imports.add(f"__relative_import_level_{node.level}__")

            for alias in node.names:
                imported_as = alias.asname if alias.asname else alias.name
                self.local_names.add(imported_as)
            self.internal_import_statements.append(ast.unparse(node))

        self.generic_visit(node)

    def visit_Name(self, node: ast.Name) -> None:
        if self.inside_function:
            if isinstance(node.ctx, ast.Load):
                self.name_references.add(node.id)
            elif isinstance(node.ctx, ast.Store):
                self.local_names.add(node.id)
        self.generic_visit(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        if self.inside_function:
            self.local_names.add(node.name)
        self.generic_visit(node)

    def visit_For(self, node: ast.For) -> None:
        if self.inside_function and isinstance(node.target, ast.Name):
            self.local_names.add(node.target.id)
        self.generic_visit(node)

    def visit_comprehension(self, node: ast.comprehension) -> None:
        if self.inside_function and isinstance(node.target, ast.Name):
            self.local_names.add(node.target.id)
        self.generic_visit(node)

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        if self.inside_function and node.name:
            self.local_names.add(node.name)
        self.generic_visit(node)

    def visit_With(self, node: ast.With) -> None:
        if self.inside_function:
            for item in node.items:
                if item.optional_vars and isinstance(item.optional_vars, ast.Name):
                    self.local_names.add(item.optional_vars.id)
        self.generic_visit(node)


def _traverse_and_collect_dependencies(func: FunctionType) -> DependencyInfo:
    """
    Traverse a function and collect its dependencies.

    Returns a dict with:
        - import_statements: List of import statements needed
        - definitions: List of (name, obj) tuples for helper functions/classes/constants
    """
    source = dedent(getsource(func))
    tree = ast.parse(source)

    visitor = _DependencyVisitor(func.__name__)
    visitor.visit(tree)

    builtin_names = set(dir(builtins))
    external_refs = (visitor.name_references - visitor.local_names) - builtin_names

    import_statements = []
    definitions = []
    visited = set()

    # Include all internal import statements
    import_statements.extend(visitor.internal_import_statements)

    # Analyze external references recursively
    def analyze_object(obj: Any, name: str, depth: int = 0) -> None:
        if depth > 20:
            return

        obj_id = id(obj)
        if obj_id in visited:
            return
        visited.add(obj_id)

        # Handle modules
        if inspect.ismodule(obj):
            import_stmt = _extract_import_statement(name, obj)
            import_statements.append(import_stmt)
            return

        # Handle functions and classes
        if (
            inspect.isfunction(obj)
            or inspect.isclass(obj)
            or inspect.isbuiltin(obj)
            or inspect.ismethod(obj)
        ):
            obj_module = getattr(obj, "__module__", None)
            if obj_module:
                base_module = obj_module.split(".")[0]
                module_category = _categorize_module(base_module)

                # If from stdlib/third-party, just add import
                if module_category in ("stdlib", "third_party"):
                    obj_name = getattr(obj, "__name__", name)

                    # Check if object is accessible by 'name' (in globals or closures)
                    is_accessible = False
                    if name in func.__globals__ and func.__globals__[name] is obj:
                        is_accessible = True
                    elif func.__closure__ and hasattr(func, "__code__"):
                        freevars = func.__code__.co_freevars
                        for i, var_name in enumerate(freevars):
                            if var_name == name and i < len(func.__closure__):
                                try:
                                    if func.__closure__[i].cell_contents is obj:
                                        is_accessible = True
                                        break
                                except (ValueError, AttributeError):
                                    pass

                    if is_accessible and name == obj_name:
                        # Direct import: from requests import get, from math import sqrt
                        import_statements.append(f"from {base_module} import {name}")
                    else:
                        # Module import: import requests
                        import_statements.append(f"import {base_module}")
                    return

            try:
                obj_tree = ast.parse(dedent(getsource(obj)))
                obj_visitor = _DependencyVisitor(obj.__name__)
                obj_visitor.visit(obj_tree)

                obj_external_refs = obj_visitor.name_references - obj_visitor.local_names
                obj_external_refs = obj_external_refs - builtin_names

                # Add internal imports from this object
                import_statements.extend(obj_visitor.internal_import_statements)

                # Recursively analyze its dependencies
                obj_globals = getattr(obj, "__globals__", None)
                obj_closure = getattr(obj, "__closure__", None)
                obj_code = getattr(obj, "__code__", None)
                if obj_globals:
                    for ref_name in obj_external_refs:
                        ref_obj = None

                        # Check globals first
                        if ref_name in obj_globals:
                            ref_obj = obj_globals[ref_name]
                        # Check closure variables using co_freevars
                        elif obj_closure and obj_code:
                            freevars = obj_code.co_freevars
                            for i, var_name in enumerate(freevars):
                                if var_name == ref_name and i < len(obj_closure):
                                    try:
                                        ref_obj = obj_closure[i].cell_contents
                                        break
                                    except (ValueError, AttributeError):
                                        pass

                        if ref_obj is not None:
                            analyze_object(ref_obj, ref_name, depth + 1)

                # Add this object to definitions
                if not inspect.ismodule(obj):
                    ref_module = getattr(obj, "__module__", None)
                    if ref_module:
                        ref_base_module = ref_module.split(".")[0]
                        ref_category = _categorize_module(ref_base_module)
                        if ref_category not in ("stdlib", "third_party"):
                            definitions.append((name, obj))
                    else:
                        definitions.append((name, obj))

            except (OSError, TypeError):
                pass
            return

        if isinstance(obj, (int, float, str, bool, list, dict, tuple, set, frozenset, type(None))):
            definitions.append((name, obj))

    # Analyze all external references
    for name in external_refs:
        obj = None

        # First check globals
        if name in func.__globals__:
            obj = func.__globals__[name]
        # Then check closure variables (sibling functions in enclosing scope)
        elif func.__closure__ and func.__code__.co_freevars:
            # Match closure variable names with cell contents
            freevars = func.__code__.co_freevars
            for i, var_name in enumerate(freevars):
                if var_name == name and i < len(func.__closure__):
                    try:
                        obj = func.__closure__[i].cell_contents
                        break
                    except (ValueError, AttributeError):
                        # Cell is empty or doesn't have contents
                        pass

        if obj is not None:
            analyze_object(obj, name)

    # Remove duplicate import statements
    unique_imports = []
    seen = set()
    for stmt in import_statements:
        if stmt not in seen:
            seen.add(stmt)
            unique_imports.append(stmt)

    # Remove duplicate definitions
    unique_definitions = []
    seen_names = set()
    for name, obj in definitions:
        if name not in seen_names:
            seen_names.add(name)
            unique_definitions.append((name, obj))

    return {
        "import_statements": unique_imports,
        "definitions": unique_definitions,
    }


def generate_source_code(func: FunctionType) -> str:
    """
    Generate complete source code for a function with all dependencies.

    Args:
        func: The function to generate source code for

    Returns:
        Complete Python source code as a string
    """

    if func in _function_dependency_map:
        info = _function_dependency_map[func]
    else:
        info = _traverse_and_collect_dependencies(func)
        _function_dependency_map[func] = info

    # Build source code
    parts = []

    # 1. Add imports
    if info["import_statements"]:
        parts.append("\n".join(info["import_statements"]))

    # 2. Add definitions
    for name, obj in info["definitions"]:
        try:
            if inspect.isfunction(obj):
                source = dedent(getsource(obj))
                tree = ast.parse(source)
                if tree.body and isinstance(tree.body[0], (ast.FunctionDef, ast.AsyncFunctionDef)):
                    tree.body[0].decorator_list = []
                    source = ast.unparse(tree)
                parts.append(source)
            elif inspect.isclass(obj):
                source = dedent(getsource(obj))
                tree = ast.parse(source)
                if tree.body and isinstance(tree.body[0], ast.ClassDef):
                    tree.body[0].decorator_list = []
                    source = ast.unparse(tree)
                parts.append(source)
            else:
                parts.append(f"{name} = {repr(obj)}")
        except (OSError, TypeError):
            pass

    # 3. Add main function (without decorators)
    func_source = dedent(getsource(func))
    tree = ast.parse(func_source)
    if tree.body and isinstance(tree.body[0], (ast.FunctionDef, ast.AsyncFunctionDef)):
        tree.body[0].decorator_list = []
        func_source = ast.unparse(tree)
    parts.append(func_source)

    return "\n\n".join(parts)

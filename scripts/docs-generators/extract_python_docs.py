#!/usr/bin/env python3
"""
Extract Python API documentation using griffe.

This script extracts structured documentation from Python packages
without requiring them to be installed or imported.

Usage:
    python extract_python_docs.py <package_path> <package_name>

Example:
    python extract_python_docs.py libs/python/computer/computer computer
"""

import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

from griffe import (
    Alias,
    Attribute,
    Class,
    DocstringSectionKind,
    Function,
    Module,
    Object,
    load,
)


def extract_docstring_sections(obj: Object) -> Dict[str, Any]:
    """Extract parsed docstring sections from an object."""
    result = {
        "description": "",
        "params": [],
        "returns": None,
        "raises": [],
        "examples": [],
    }

    if not obj.docstring:
        return result

    # Get parsed sections (griffe parses automatically based on detected style)
    try:
        sections = obj.docstring.parsed
    except Exception:
        # Fallback to raw docstring
        result["description"] = str(obj.docstring.value) if obj.docstring else ""
        return result

    for section in sections:
        if section.kind == DocstringSectionKind.text:
            result["description"] = str(section.value).strip()

        elif section.kind == DocstringSectionKind.parameters:
            for param in section.value:
                result["params"].append({
                    "name": param.name,
                    "type": str(param.annotation) if param.annotation else "",
                    "description": param.description or "",
                    "default": str(param.default) if param.default else None,
                })

        elif section.kind == DocstringSectionKind.returns:
            ret = section.value
            if ret:
                result["returns"] = {
                    "type": str(ret.annotation) if hasattr(ret, 'annotation') and ret.annotation else "",
                    "description": ret.description if hasattr(ret, 'description') else str(ret),
                }

        elif section.kind == DocstringSectionKind.raises:
            for exc in section.value:
                result["raises"].append({
                    "type": str(exc.annotation) if exc.annotation else "",
                    "description": exc.description or "",
                })

        elif section.kind == DocstringSectionKind.examples:
            result["examples"].append(str(section.value))

    return result


def extract_function(fn: Function, is_method: bool = False) -> Dict[str, Any]:
    """Extract function/method documentation."""
    # Check if async (stored in labels)
    is_async = 'async' in getattr(fn, 'labels', set())

    # Build signature
    params = []
    for param in fn.parameters:
        param_str = param.name
        if param.annotation:
            param_str += f": {param.annotation}"
        if param.default is not None:
            param_str += f" = {param.default}"
        params.append(param_str)

    signature = f"{'async ' if is_async else ''}def {fn.name}({', '.join(params)})"
    if fn.returns:
        signature += f" -> {fn.returns}"

    docstring_data = extract_docstring_sections(fn)

    return {
        "name": fn.name,
        "signature": signature,
        "is_async": is_async,
        "is_method": is_method,
        "description": docstring_data["description"],
        "parameters": docstring_data["params"],
        "returns": docstring_data["returns"],
        "raises": docstring_data["raises"],
        "examples": docstring_data["examples"],
        "is_private": fn.name.startswith("_") and not fn.name.startswith("__"),
        "is_dunder": fn.name.startswith("__") and fn.name.endswith("__"),
    }


def extract_attribute(attr: Attribute) -> Dict[str, Any]:
    """Extract attribute/property documentation."""
    return {
        "name": attr.name,
        "type": str(attr.annotation) if attr.annotation else "",
        "description": str(attr.docstring.value) if attr.docstring else "",
        "default": str(attr.value) if attr.value else None,
        "is_private": attr.name.startswith("_"),
    }


def resolve_member(member: Any) -> Any:
    """Resolve an alias to its target if needed."""
    if isinstance(member, Alias):
        try:
            return member.target
        except Exception:
            return member
    return member


def extract_class(cls: Class) -> Dict[str, Any]:
    """Extract class documentation."""
    docstring_data = extract_docstring_sections(cls)

    # Extract methods
    methods = []
    for name, member in cls.members.items():
        member = resolve_member(member)
        if isinstance(member, Function):
            method_doc = extract_function(member, is_method=True)
            # Skip private methods except __init__
            if not method_doc["is_private"] or name == "__init__":
                methods.append(method_doc)

    # Extract attributes/properties
    attributes = []
    for name, member in cls.members.items():
        member = resolve_member(member)
        if isinstance(member, Attribute) and not name.startswith("_"):
            attributes.append(extract_attribute(member))

    # Get base classes
    bases = []
    for base in cls.bases:
        bases.append(str(base))

    return {
        "name": cls.name,
        "description": docstring_data["description"],
        "bases": bases,
        "methods": methods,
        "attributes": attributes,
        "is_private": cls.name.startswith("_"),
    }


def extract_module(module: Module, include_private: bool = False) -> Dict[str, Any]:
    """Extract module documentation."""
    # Get version from module if available
    version = "unknown"
    if "__version__" in module.members:
        version_attr = module.members["__version__"]
        if hasattr(version_attr, "value") and version_attr.value:
            version = str(version_attr.value).strip("'\"")

    # Get __all__ exports if defined
    exports = None
    if "__all__" in module.members:
        all_attr = module.members["__all__"]
        if hasattr(all_attr, "value") and all_attr.value:
            try:
                # Try to parse __all__ as a list
                exports = eval(str(all_attr.value))
            except Exception:
                exports = None

    # Extract classes
    classes = []
    for name, member in module.members.items():
        member = resolve_member(member)
        if isinstance(member, Class):
            # Include if in __all__ or if public
            if exports is None:
                if not name.startswith("_") or include_private:
                    classes.append(extract_class(member))
            elif name in exports:
                classes.append(extract_class(member))

    # Extract module-level functions
    functions = []
    for name, member in module.members.items():
        member = resolve_member(member)
        if isinstance(member, Function):
            if exports is None:
                if not name.startswith("_") or include_private:
                    functions.append(extract_function(member))
            elif name in exports:
                functions.append(extract_function(member))

    return {
        "name": module.name,
        "version": version,
        "docstring": str(module.docstring.value) if module.docstring else "",
        "exports": exports,
        "classes": classes,
        "functions": functions,
    }


def extract_package_docs(package_path: str, package_name: str) -> Dict[str, Any]:
    """
    Extract documentation from a Python package.

    Args:
        package_path: Path to the package directory (e.g., 'libs/python/computer/computer')
        package_name: Name of the package to load (e.g., 'computer')

    Returns:
        Dictionary containing structured documentation
    """
    # Resolve paths
    package_dir = Path(package_path).resolve()
    search_path = package_dir.parent

    # Load the package using griffe
    try:
        package = load(
            package_name,
            search_paths=[str(search_path)],
        )
    except Exception as e:
        return {
            "error": f"Failed to load package: {e}",
            "name": package_name,
            "version": "unknown",
            "modules": [],
        }

    # Extract main module
    main_module = extract_module(package)

    # Extract submodules
    submodules = []
    for name, member in package.members.items():
        member = resolve_member(member)
        if isinstance(member, Module) and not name.startswith("_"):
            submodule_doc = extract_module(member)
            if submodule_doc["classes"] or submodule_doc["functions"]:
                submodules.append(submodule_doc)

    return {
        "name": package_name,
        "version": main_module["version"],
        "docstring": main_module["docstring"],
        "exports": main_module["exports"],
        "classes": main_module["classes"],
        "functions": main_module["functions"],
        "submodules": submodules,
    }


def main():
    """Main entry point."""
    if len(sys.argv) < 3:
        print("Usage: python extract_python_docs.py <package_path> <package_name>", file=sys.stderr)
        print("Example: python extract_python_docs.py libs/python/computer/computer computer", file=sys.stderr)
        sys.exit(1)

    package_path = sys.argv[1]
    package_name = sys.argv[2]

    docs = extract_package_docs(package_path, package_name)
    print(json.dumps(docs, indent=2))


if __name__ == "__main__":
    main()

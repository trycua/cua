#!/usr/bin/env python3
"""The public surface of an installed Python SDK as JSON: what a user can
import from the package root, with signatures, base classes, enum members and
model fields. The drift workflow diffs the latest release against the
committed fixture of the newest claimed version (surface_diff.py).

    python surface_py.py daytona > fixtures/daytona-py-0.225.0.json
    python surface_py.py modal modal.exception

Deterministic: sorted, no addresses, no defaults that are objects.
"""

from __future__ import annotations

import enum
import importlib
import importlib.metadata as md
import inspect
import json
import re
import sys


def _sig(obj) -> str:
    try:
        s = str(inspect.signature(obj))
    except (TypeError, ValueError):
        return "(?)"
    return re.sub(r" at 0x[0-9a-f]+", "", s)


def _ann(a) -> str:
    if a is inspect.Parameter.empty:
        return ""
    return (
        a if isinstance(a, str) else getattr(a, "__name__", None) or re.sub(r"typing\.", "", str(a))
    )


def _fields(cls) -> dict:
    out = {}
    mf = getattr(cls, "model_fields", None)  # pydantic v2
    if isinstance(mf, dict):
        for k, v in sorted(mf.items()):
            out[k] = _ann(getattr(v, "annotation", ""))
        return out
    f = getattr(cls, "__dataclass_fields__", None)
    if f:
        for k, v in sorted(f.items()):
            out[k] = _ann(v.type)
        return out
    ann = getattr(cls, "__annotations__", {})
    for k in sorted(ann):
        if not k.startswith("_"):
            out[k] = _ann(ann[k])
    return out


def describe_class(cls) -> dict:
    d: dict = {
        "kind": "class",
        "bases": sorted(b.__name__ for b in cls.__bases__ if b is not object),
    }
    if issubclass(cls, enum.Enum):
        d["kind"] = "enum"
        d["members"] = {m.name: repr(m.value) for m in cls}
        return d
    if issubclass(cls, BaseException):
        d["kind"] = "exception"
    d["init"] = _sig(cls)
    d["fields"] = _fields(cls)
    methods = {}
    for name, m in sorted(vars(cls).items()):
        if name.startswith("_") and name != "__init__":
            continue
        raw = m.__func__ if isinstance(m, (classmethod, staticmethod)) else m
        if isinstance(m, property):
            methods[name] = {"property": True}
        elif inspect.isfunction(raw) or inspect.ismethoddescriptor(raw):
            if name == "__init__":
                continue
            methods[name] = {
                "sig": _sig(raw),
                "async": inspect.iscoroutinefunction(raw) or inspect.isasyncgenfunction(raw),
                "kind": (
                    "classmethod"
                    if isinstance(m, classmethod)
                    else "staticmethod" if isinstance(m, staticmethod) else "method"
                ),
            }
    d["methods"] = methods
    return d


def dump(modules: list[str]) -> dict:
    out: dict = {"package": modules[0], "version": None, "symbols": {}}
    try:
        out["version"] = md.version(modules[0].split(".")[0].replace("_", "-"))
    except md.PackageNotFoundError:
        pass
    for modname in modules:
        mod = importlib.import_module(modname)
        names = getattr(mod, "__all__", None) or [n for n in dir(mod) if not n.startswith("_")]
        for n in sorted(set(names)):
            if not hasattr(mod, n):
                out["symbols"][f"{modname}.{n}"] = {"kind": "missing-export"}
                continue
            o = getattr(mod, n)
            key = f"{modname}.{n}"
            if inspect.isclass(o):
                out["symbols"][key] = describe_class(o)
            elif inspect.isfunction(o) or inspect.isbuiltin(o):
                out["symbols"][key] = {
                    "kind": "function",
                    "sig": _sig(o),
                    "async": inspect.iscoroutinefunction(o),
                }
            elif inspect.ismodule(o):
                out["symbols"][key] = {"kind": "module"}
            else:
                out["symbols"][key] = {"kind": "value", "type": type(o).__name__}
    return out


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    print(json.dumps(dump(sys.argv[1:]), indent=1, sort_keys=True))

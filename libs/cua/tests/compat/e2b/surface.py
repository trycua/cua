"""The public surface of the installed `e2b` SDK, as JSON: classes, methods
and their signatures, exception classes, and the REST/envd routes its
generated clients call. The weekly drift job diffs the latest SDK's surface
with the committed one, so a new or changed call is noticed the week it
ships.

    python surface.py > surface.json
    python surface.py --diff fixtures/surface-2.56.0.json
"""

from __future__ import annotations

import argparse
import importlib
import inspect
import json
import pkgutil
import re
import sys
from pathlib import Path

import e2b

CLASSES = [
    "e2b.AsyncSandbox",
    "e2b.Sandbox",
    "e2b.sandbox_async.commands.command.Commands",
    "e2b.sandbox_async.commands.command_handle.AsyncCommandHandle",
    "e2b.sandbox_async.commands.pty.Pty",
    "e2b.sandbox_async.filesystem.filesystem.Filesystem",
    "e2b.sandbox_sync.commands.command.Commands",
    "e2b.sandbox_sync.filesystem.filesystem.Filesystem",
]


def resolve(path: str):
    mod, _, name = path.rpartition(".")
    return getattr(importlib.import_module(mod), name)


def sig(obj) -> str:
    try:
        s = str(inspect.signature(obj))
    except (TypeError, ValueError):
        return "?"
    # Defaults that are object reprs carry addresses.
    return re.sub(r" at 0x[0-9a-f]+", "", s)


def describe(obj) -> dict:
    """One exported name: its kind and, for classes, every public member
    (methods with signatures, properties, dataclass/attrs/TypedDict fields,
    enum members)."""
    import enum

    if inspect.isclass(obj):
        d: dict = {"kind": "class", "bases": [b.__name__ for b in obj.__bases__]}
        members: dict = {}
        if issubclass(obj, enum.Enum):
            d["kind"] = "enum"
            members = {m.name: repr(m.value) for m in obj}
        else:
            for name, member in inspect.getmembers(obj):
                if name.startswith("_") and name != "__init__":
                    continue
                if isinstance(member, property):
                    members[name] = "property"
                elif callable(member):
                    members[name] = sig(member)
            ann = {}
            for klass in reversed(obj.__mro__):
                ann.update(getattr(klass, "__annotations__", {}) or {})
            for name, t in ann.items():
                if not name.startswith("_") and name not in members:
                    members[name] = "field: " + re.sub(r" at 0x[0-9a-f]+", "", str(t))
        d["members"] = members
        return d
    if callable(obj):
        return {"kind": "function", "signature": sig(obj)}
    if inspect.ismodule(obj):
        return {"kind": "module"}
    return {"kind": "value", "type": type(obj).__name__}


EXTRA = [
    "e2b.sandbox_async.commands.command_handle.AsyncCommandHandle",
    "e2b.sandbox_async.commands.command_input_handle.AsyncCommandInputHandle",
    "e2b.sandbox_sync.commands.command_handle.CommandHandle",
    "e2b.sandbox_async.filesystem.watch_handle.AsyncWatchHandle",
    "e2b.sandbox_sync.filesystem.watch_handle.WatchHandle",
    "e2b.sandbox_sync.commands.pty.Pty",
    "e2b.sandbox.mcp.McpServer",
]


def surface() -> dict:
    out: dict = {"classes": {}, "exceptions": {}, "routes": [], "exports": {}}
    for name in sorted(dir(e2b)):
        if not name.startswith("_"):
            out["exports"]["e2b." + name] = describe(getattr(e2b, name))
    for path in EXTRA:
        try:
            out["exports"][path] = describe(resolve(path))
        except Exception as e:  # noqa: BLE001
            out["exports"][path] = {"kind": "missing", "error": type(e).__name__}
    for path in CLASSES:
        try:
            cls = resolve(path)
        except Exception as e:  # noqa: BLE001
            out["classes"][path] = {"missing": type(e).__name__}
            continue
        methods = {}
        for name, member in inspect.getmembers(cls):
            if name.startswith("_") and name not in ("__init__",):
                continue
            if callable(member):
                methods[name] = sig(member)
            elif isinstance(member, property):
                methods[name] = "property"
        out["classes"][path] = methods
    for name in dir(e2b):
        obj = getattr(e2b, name)
        if inspect.isclass(obj) and issubclass(obj, BaseException):
            out["exceptions"][name] = [
                c.__name__ for c in obj.__mro__ if c.__module__.startswith("e2b")
            ]
    # Every route the generated REST client and the envd stubs can call.
    import e2b.api.client.api as api_pkg

    for m in pkgutil.walk_packages(api_pkg.__path__, api_pkg.__name__ + "."):
        src = Path(importlib.import_module(m.name).__file__).read_text()
        for method, url in re.findall(r'"method": "(\w+)",\s*"url": f?"([^"]+)"', src):
            out["routes"].append(f"{method.upper()} {url}")
    for svc in ("e2b.envd.process.process_connect", "e2b.envd.filesystem.filesystem_connect"):
        src = Path(importlib.import_module(svc).__file__).read_text()
        out["routes"] += sorted({f"POST {p}" for p in re.findall(r'"(/\w+\.\w+/\w+)"', src)})
    out["routes"] = sorted(set(out["routes"]))
    return out


def diff(old: dict, new: dict) -> list[str]:
    d = []
    for path in sorted(set(old["classes"]) | set(new["classes"])):
        a, b = old["classes"].get(path, {}), new["classes"].get(path, {})
        for m in sorted(set(a) | set(b)):
            if a.get(m) != b.get(m):
                d.append(f"{path}.{m}: {a.get(m)!r} -> {b.get(m)!r}")
    for name in sorted(set(old["exceptions"]) | set(new["exceptions"])):
        if old["exceptions"].get(name) != new["exceptions"].get(name):
            d.append(
                f"exception {name}: {old['exceptions'].get(name)} -> {new['exceptions'].get(name)}"
            )
    oe, ne = old.get("exports"), new.get("exports", {})
    if oe is not None:
        for name in sorted(set(oe) | set(ne)):
            a, b = oe.get(name), ne.get(name)
            if a is None or b is None:
                d.append(("+ " if a is None else "- ") + "export " + name)
            elif a != b:
                am, bm = a.get("members", {}), b.get("members", {})
                for m in sorted(set(am) | set(bm)):
                    if am.get(m) != bm.get(m):
                        d.append(f"{name}.{m}: {am.get(m)!r} -> {bm.get(m)!r}")
                for k in ("signature", "bases", "kind", "type"):
                    if a.get(k) != b.get(k):
                        d.append(f"{name} {k}: {a.get(k)!r} -> {b.get(k)!r}")
    for r in sorted(set(old["routes"]) ^ set(new["routes"])):
        d.append(("+ " if r in new["routes"] else "- ") + "route " + r)
    return d


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--diff")
    a = ap.parse_args()
    now = surface()
    if not a.diff:
        print(json.dumps(now, indent=1, sort_keys=True))
        sys.exit(0)
    changes = diff(json.loads(Path(a.diff).read_text()), now)
    if changes:
        print(f"{len(changes)} change(s) in the e2b SDK surface since {a.diff}:")
        for c in changes:
            print("  " + c)
        sys.exit(1)
    print(f"surface unchanged since {a.diff}")

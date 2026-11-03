"""The e2b public API surface cua-e2b covers, as data: dump it from an `e2b`
module (the real package or cua-e2b) and compare two dumps.

    python -m conformance.api > api.json

A covered callable must accept every parameter the real one has, with the
same kind and default (extra parameters need defaults); covered dataclasses
must have the real fields; covered exceptions the real bases. Return
annotations are not compared (cua-e2b returns its own classes of the same
shape).
"""

from __future__ import annotations

import dataclasses
import importlib
import inspect
import json
import sys

# (module, class, [methods]); methods that are properties are listed too.
CALLABLES = [
    (
        "e2b",
        "AsyncSandbox",
        [
            "create",
            "connect",
            "kill",
            "set_timeout",
            "get_host",
            "get_info",
            "get_metrics",
            "pause",
            "create_snapshot",
            "list",
            "is_running",
            "fork",
            "list_snapshots",
            "delete_snapshot",
            "beta_pause",
        ],
    ),
    (
        "e2b",
        "Sandbox",
        [
            "create",
            "connect",
            "kill",
            "set_timeout",
            "get_host",
            "get_info",
            "get_metrics",
            "pause",
            "create_snapshot",
            "list",
            "is_running",
            "fork",
            "list_snapshots",
            "delete_snapshot",
            "beta_pause",
        ],
    ),
    (
        "e2b.sandbox_async.commands.command",
        "Commands",
        ["run", "kill", "list", "send_stdin", "close_stdin", "connect"],
    ),
    (
        "e2b.sandbox_async.commands.pty",
        "Pty",
        ["create", "connect", "send_stdin", "resize", "kill"],
    ),
    (
        "e2b.sandbox_async.filesystem.filesystem",
        "Filesystem",
        [
            "read",
            "write",
            "write_files",
            "list",
            "exists",
            "get_info",
            "remove",
            "rename",
            "make_dir",
            "watch_dir",
        ],
    ),
    (
        "e2b",
        "AsyncCommandHandle",
        [
            "wait",
            "kill",
            "disconnect",
            "send_stdin",
            "close_stdin",
            "pid",
            "stdout",
            "stderr",
            "exit_code",
            "error",
        ],
    ),
    ("e2b", "CommandHandle", ["wait", "kill", "disconnect", "send_stdin"]),
    ("e2b", "AsyncWatchHandle", ["stop"]),
    (
        "e2b.sandbox_async.git",
        "Git",
        [
            "clone",
            "init",
            "remote_add",
            "remote_get",
            "status",
            "branches",
            "create_branch",
            "checkout_branch",
            "delete_branch",
            "add",
            "commit",
            "reset",
            "restore",
            "push",
            "pull",
            "set_config",
            "get_config",
            "dangerously_authenticate",
            "configure_user",
        ],
    ),
    ("e2b", "WatchHandle", ["get_new_events", "stop"]),
    (
        "e2b",
        "AsyncTemplate",
        ["build", "build_in_background", "get_build_status", "exists", "alias_exists"],
    ),
    ("e2b", "Template", ["build", "build_in_background", "get_build_status", "exists"]),
    (
        "e2b.template.main",
        "TemplateBuilder",
        [
            "run_cmd",
            "copy",
            "copy_items",
            "remove",
            "rename",
            "make_dir",
            "make_symlink",
            "set_workdir",
            "set_user",
            "set_envs",
            "pip_install",
            "npm_install",
            "bun_install",
            "apt_install",
            "git_clone",
            "set_start_cmd",
            "set_ready_cmd",
            "skip_cache",
        ],
    ),
    (
        "e2b.template.main",
        "TemplateBase",
        ["from_image", "from_template", "from_dockerfile", "from_base_image", "from_python_image"],
    ),
    (
        "e2b",
        "AsyncVolume",
        [
            "create",
            "connect",
            "destroy",
            "exists",
            "make_dir",
            "read_file",
            "remove",
            "update_metadata",
            "write_file",
            "volume_id",
            "name",
            "token",
        ],
    ),
    ("e2b", "Volume", ["create", "connect", "destroy", "write_file", "read_file"]),
]
DATACLASSES = [
    "CommandResult",
    "CommandExitException",
    "EntryInfo",
    "WriteInfo",
    "PtySize",
    "ProcessInfo",
    "FilesystemEvent",
    "SnapshotInfo",
    "SandboxMetrics",
]
EXCEPTIONS = [
    "SandboxException",
    "TimeoutException",
    "InvalidArgumentException",
    "NotEnoughSpaceException",
    "NotFoundException",
    "FileNotFoundException",
    "SandboxNotFoundException",
    "AuthenticationException",
    "TemplateException",
    "RateLimitException",
    "CommandExitException",
    # e2b 2.56
    "SandboxNotRunningException",
    "SandboxUnreachableException",
    "ServiceBusyException",
]
ENUMS = ["FileType", "FilesystemEventType"]


def _sig(fn) -> list:
    target = getattr(fn, "method", fn)  # e2b's class_method_variant
    target = getattr(target, "__func__", target)
    try:
        sig = inspect.signature(target)
    except (TypeError, ValueError):
        return ["<no signature>"]
    return [
        [p.name, p.kind.name, None if p.default is inspect.Parameter.empty else repr(p.default)]
        for p in sig.parameters.values()
        if p.name not in ("self", "cls")
    ]


def dump(e2b_name: str = "e2b") -> dict:
    base = importlib.import_module(e2b_name)
    out: dict = {"callables": {}, "dataclasses": {}, "exceptions": {}, "enums": {}}
    for mod_name, cls_name, members in CALLABLES:
        cls = getattr(importlib.import_module(mod_name), cls_name)
        for m in members:
            raw = inspect.getattr_static(cls, m)
            key = f"{cls_name}.{m}"
            if isinstance(raw, property):
                out["callables"][key] = "property"
            elif isinstance(raw, (staticmethod, classmethod)):
                out["callables"][key] = _sig(raw.__func__)
            else:
                out["callables"][key] = _sig(raw)
    for name in DATACLASSES:
        cls = getattr(base, name, None) or getattr(
            importlib.import_module("e2b.sandbox.sandbox_api"), name, None
        )
        out["dataclasses"][name] = sorted(f.name for f in dataclasses.fields(cls)) if cls else None
    for name in EXCEPTIONS:
        cls = getattr(base, name, None)
        if cls is None:  # a release that predates it
            continue
        out["exceptions"][name] = [b.__name__ for b in cls.__mro__[1:] if b.__name__ != "object"]
    for name in ENUMS:
        out["enums"][name] = sorted(m.value for m in getattr(base, name))
    return out


# e2b 2.56 changed these defaults from True to None ("the platform default",
# which is True). cua-e2b takes the newer spelling and treats None as True, so
# both releases' goldens are met.
EQUIVALENT_DEFAULTS = {
    ("create", "secure"): {"True", "None"},
    ("create", "allow_internet_access"): {"True", "None"},
    ("pause", "keep_memory"): {"True", "None"},
    ("beta_pause", "keep_memory"): {"True", "None"},
    ("get_build_status", "logs_offset"): {"None", "0"},
    # 2.42 spelled the template build size 2 vCPU / 1024 MiB; 2.56 leaves it to the platform
    ("build", "cpu_count"): {"None", "2"},
    ("build", "memory_mb"): {"None", "1024"},
    ("build_in_background", "cpu_count"): {"None", "2"},
    ("build_in_background", "memory_mb"): {"None", "1024"},
}


def _same_default(key: str, name: str, have: str, want: str) -> bool:
    if have == want:
        return True
    group = EQUIVALENT_DEFAULTS.get((key.split(".")[-1], name))
    return bool(group and have in group and want in group)


def compare(golden: dict, actual: dict) -> list:
    problems = []
    for key, gsig in golden["callables"].items():
        asig = actual["callables"].get(key)
        if asig is None:
            problems.append(f"{key}: missing")
            continue
        if gsig == "property" or asig == "property":
            if gsig != asig:
                problems.append(f"{key}: property mismatch ({gsig!r} vs {asig!r})")
            continue
        have = {p[0]: p for p in asig}
        names = [p[0] for p in asig if p[1] in ("POSITIONAL_ONLY", "POSITIONAL_OR_KEYWORD")]
        gnames = [p[0] for p in gsig if p[1] in ("POSITIONAL_ONLY", "POSITIONAL_OR_KEYWORD")]
        if names[: len(gnames)] != gnames:
            problems.append(f"{key}: positional parameters {names} do not start with {gnames}")
        for name, kind, default in gsig:
            if kind in ("VAR_KEYWORD", "VAR_POSITIONAL"):
                continue
            if name not in have:
                problems.append(f"{key}: no parameter {name!r}")
                continue
            if not _same_default(key, name, have[name][2], default):
                problems.append(f"{key}({name}): default {have[name][2]} != {default}")
        for name, kind, default in asig:
            gp = {p[0] for p in gsig}
            if name not in gp and default is None and kind not in ("VAR_KEYWORD", "VAR_POSITIONAL"):
                problems.append(f"{key}: extra required parameter {name!r}")
    for name, fields in golden["dataclasses"].items():
        have = set(actual["dataclasses"].get(name) or [])
        missing = set(fields or []) - have
        if missing:
            problems.append(f"{name}: missing fields {sorted(missing)}")
    for name, bases in golden["exceptions"].items():
        if actual["exceptions"].get(name) != bases:
            problems.append(f"{name}: bases {actual['exceptions'].get(name)} != {bases}")
    for name, values in golden["enums"].items():
        if actual["enums"].get(name) != values:
            problems.append(f"{name}: values {actual['enums'].get(name)} != {values}")
    return problems


if __name__ == "__main__":
    print(json.dumps(dump(sys.argv[1] if len(sys.argv) > 1 else "e2b"), indent=1, sort_keys=True))

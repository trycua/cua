"""The whole public surface of an `e2b` package, as data: every public class,
function, method, dataclass field, enum value and exception base in every
module of the package, dumped from the real SDK (a release) or from cua-e2b,
and compared.

    python -m conformance.surface > e2b-2.58.0-surface.json     # run in an env with that e2b
    python -m conformance.surface --compare golden.json          # cua-e2b vs a recorded surface

The goldens are `golden/e2b-<version>-surface.json`. `compare()` returns what
is missing or different; `tests/test_surface.py` fails on any difference that
`surface_waivers.WAIVERS` does not name with a reason, and `COVERAGE.md` is
generated from the same data (`python -m conformance.matrix`).

Not part of the surface (generated or transport code, no counterpart here):
`INTERNAL_PREFIXES`.
"""

from __future__ import annotations

import dataclasses
import enum
import importlib
import inspect
import json
import pathlib
import re
import sys
from typing import Any, Dict, List, Optional

# Generated REST/envd clients and transport internals. Their public classes
# (`e2b.api.client.models.*`, `e2b.envd.*`) are the wire format of E2B's own
# control plane, which cua-e2b does not speak.
INTERNAL_PREFIXES = (
    "e2b.api.client",
    "e2b.envd",
    "e2b.volume.client",
)


def _repr(value) -> str:
    """A default's repr without the memory address."""
    return re.sub(r" at 0x[0-9a-f]+", "", repr(value))


def _sig(fn) -> Optional[list]:
    target = getattr(fn, "method", fn)  # e2b's class_method_variant
    target = getattr(target, "__func__", target)
    try:
        sig = inspect.signature(target)
    except (TypeError, ValueError):
        return None
    return [
        [p.name, p.kind.name, None if p.default is inspect.Parameter.empty else _repr(p.default)]
        for p in sig.parameters.values()
        if p.name not in ("self", "cls")
    ]


def _members(cls) -> Dict[str, Any]:
    out: Dict[str, Any] = {}
    for name in dir(cls):
        if name.startswith("_") and name != "__init__":
            continue
        try:
            raw = inspect.getattr_static(cls, name)
        except AttributeError:
            continue
        func = getattr(raw, "__func__", raw)
        origin = getattr(func, "__module__", "") or ""
        if name == "__init__" and not origin.startswith("e2b"):
            continue
        if isinstance(raw, property):
            out[name] = "property"
        elif isinstance(raw, (staticmethod, classmethod)):
            out[name] = _sig(raw.__func__)
        elif callable(raw) or hasattr(raw, "method"):
            out[name] = _sig(raw)
        elif isinstance(raw, (str, int, float, bool, type(None))):
            out[name] = "const:" + repr(raw)
    return out


def _module_names(root: str) -> List[str]:
    pkg = importlib.import_module(root)
    base = pathlib.Path(pkg.__path__[0])
    names = []
    for p in sorted(base.rglob("*.py")):
        parts = list(p.relative_to(base.parent).with_suffix("").parts)
        if parts[-1] == "__init__":
            parts = parts[:-1]
        names.append(".".join(parts))
    return names


def dump(root: str = "e2b") -> dict:
    out: Dict[str, dict] = {}
    for mod_name in _module_names(root):
        if any(mod_name.startswith(p) for p in INTERNAL_PREFIXES):
            continue
        try:
            mod = importlib.import_module(mod_name)
        except Exception as e:  # noqa: BLE001
            out[mod_name] = {"__import_error__": f"{type(e).__name__}: {e}"[:120]}
            continue
        items: Dict[str, dict] = {}
        for name, obj in vars(mod).items():
            if name.startswith("_") or getattr(obj, "__module__", None) != mod_name:
                continue
            if inspect.isclass(obj):
                d: Dict[str, Any] = {
                    "kind": "class",
                    "bases": [b.__name__ for b in obj.__mro__[1:] if b is not object],
                    "members": _members(obj),
                }
                if issubclass(obj, enum.Enum):
                    d["kind"] = "enum"
                    d["values"] = sorted(str(m.value) for m in obj)
                elif dataclasses.is_dataclass(obj):
                    d["kind"] = "dataclass"
                    d["fields"] = [
                        [
                            f.name,
                            (
                                "required"
                                if f.default is dataclasses.MISSING
                                and f.default_factory is dataclasses.MISSING
                                else "optional"
                            ),
                        ]
                        for f in dataclasses.fields(obj)
                    ]
                elif hasattr(obj, "__required_keys__"):  # TypedDict
                    d["kind"] = "typeddict"
                    d["keys"] = sorted(obj.__annotations__)
                    d["required"] = sorted(obj.__required_keys__)
                items[name] = d
            elif inspect.isfunction(obj) or inspect.isbuiltin(obj):
                items[name] = {"kind": "function", "sig": _sig(obj)}
        if items:
            out[mod_name] = items
        # Names a package re-exports (`e2b.__all__`).
        exported = getattr(mod, "__all__", None)
        if exported is not None:
            out.setdefault(mod_name, {})["__all__"] = sorted(exported)
    return out


# Defaults that differ between e2b releases but mean the same thing; cua-e2b
# takes the newer spelling.
EQUIVALENT_DEFAULTS = {
    ("create", "secure"): {"True", "None"},
    ("create", "allow_internet_access"): {"True", "None"},
    ("pause", "keep_memory"): {"True", "None"},
    ("beta_pause", "keep_memory"): {"True", "None"},
    ("get_build_status", "logs_offset"): {"None", "0"},
    ("build", "cpu_count"): {"None", "2"},
    ("build", "memory_mb"): {"None", "1024"},
    ("build_in_background", "cpu_count"): {"None", "2"},
    ("build_in_background", "memory_mb"): {"None", "1024"},
}


def _compare_sig(path: str, want, have) -> List[str]:
    if want == "property" or have == "property" or not isinstance(want, list):
        if want != have and not (
            isinstance(want, str)
            and isinstance(have, str)
            and want.startswith("const:")
            and have.startswith("const:")
        ):
            return [f"{path}: {want!r} != {have!r}"]
        return []
    if not isinstance(have, list):
        return [f"{path}: signature {have!r}, want {want!r}"]
    problems = []
    names = {p[0]: p for p in have}
    has_kw = any(p[1] == "VAR_KEYWORD" for p in have)
    for name, kind, default in want:
        if kind in ("VAR_POSITIONAL", "VAR_KEYWORD"):
            continue
        if name not in names:
            problems.append(
                f"{path}: no parameter {name!r}" + (" (only **kwargs)" if has_kw else "")
            )
            continue
        _, hkind, hdefault = names[name]
        # A parameter that is optional here but required upstream accepts every call
        # the upstream accepts.
        if hdefault != default and default is not None:
            group = EQUIVALENT_DEFAULTS.get((path.split(".")[-1], name))
            if not (group and hdefault in group and default in group):
                problems.append(f"{path}({name}): default {hdefault} != {default}")
        if (
            hkind != kind
            and not ({hkind, kind} <= {"POSITIONAL_OR_KEYWORD", "POSITIONAL_ONLY"})
            and not (hkind == "POSITIONAL_OR_KEYWORD" and kind == "KEYWORD_ONLY")
        ):
            problems.append(f"{path}({name}): kind {hkind} != {kind}")
    pos_w = [p[0] for p in want if p[1] in ("POSITIONAL_ONLY", "POSITIONAL_OR_KEYWORD")]
    pos_h = [p[0] for p in have if p[1] in ("POSITIONAL_ONLY", "POSITIONAL_OR_KEYWORD")]
    if pos_h[: len(pos_w)] != pos_w:
        problems.append(f"{path}: positional order {pos_h} != {pos_w}")
    wnames = {p[0] for p in want}
    for name, kind, default in have:
        if name not in wnames and default is None and kind not in ("VAR_POSITIONAL", "VAR_KEYWORD"):
            problems.append(f"{path}: extra required parameter {name!r}")
    return problems


def compare(real: dict, ours: dict) -> List[str]:
    """Differences of `ours` from `real` as `module.Name[.member]: reason`
    lines. Only what the real SDK has is checked (extras are fine)."""
    problems: List[str] = []
    for mod, items in real.items():
        if "__import_error__" in items:
            continue
        have_mod = ours.get(mod)
        if have_mod is None:
            problems.append(f"{mod}: module missing")
            continue
        for name, want in items.items():
            if name == "__all__":
                missing = sorted(set(want) - set(have_mod.get("__all__", [])))
                if missing:
                    problems.append(f"{mod}.__all__: missing {missing}")
                continue
            have = have_mod.get(name)
            if have is None:
                problems.append(f"{mod}.{name}: missing")
                continue
            if want["kind"] == "function":
                problems += _compare_sig(f"{mod}.{name}", want["sig"], have.get("sig"))
                continue
            wk, hk = want["kind"], have["kind"]
            if wk != hk and not ({wk, hk} <= {"class", "dataclass", "typeddict"}):
                problems.append(f"{mod}.{name}: kind {hk} != {wk}")
            missing_bases = [
                b
                for b in want["bases"]
                if b not in have["bases"] and b not in ("Generic", "TypedDict", "dict", "Protocol")
            ]
            if missing_bases:
                problems.append(f"{mod}.{name}: bases {have['bases']} lack {missing_bases}")
            if wk == "enum" and hk == "enum" and want["values"] != have["values"]:
                problems.append(f"{mod}.{name}: values {have['values']} != {want['values']}")
            if wk == "dataclass" and hk == "dataclass":
                wf = {f[0]: f[1] for f in want["fields"]}
                hf = {f[0]: f[1] for f in have["fields"]}
                for f in sorted(set(wf) - set(hf)):
                    problems.append(f"{mod}.{name}: missing field {f}")
                for f in sorted(set(hf) - set(wf)):
                    if hf[f] == "required":
                        problems.append(f"{mod}.{name}: extra required field {f}")
                for f in sorted(set(wf) & set(hf)):
                    if wf[f] == "optional" and hf[f] == "required":
                        problems.append(f"{mod}.{name}: field {f} is required, optional upstream")
            if wk == "typeddict" and hk == "typeddict":
                for k in sorted(set(want["keys"]) - set(have["keys"])):
                    problems.append(f"{mod}.{name}: missing key {k}")
            for m, wsig in want.get("members", {}).items():
                if m not in have.get("members", {}):
                    problems.append(f"{mod}.{name}.{m}: missing")
                    continue
                problems += _compare_sig(f"{mod}.{name}.{m}", wsig, have["members"][m])
    return problems


def main(argv: List[str]) -> int:
    if "--compare" in argv:
        golden = json.loads(pathlib.Path(argv[argv.index("--compare") + 1]).read_text())
        for line in compare(golden, dump()):
            print(line)
        return 0
    print(json.dumps(dump(), indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

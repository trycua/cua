"""The agent-desktop tool catalog, derived from the CLI's own command tree.

agent-desktop prints no machine-readable schema, but every command answers ``--help`` in a regular layout (a summary,
``Commands:``, ``Arguments:`` and ``Options:`` blocks). ``derive`` walks that tree through a ``help_for`` callable that
returns one command's help text, and turns each leaf command into one tool:

* the name is ``desktop_<command>``, with subcommands joined by ``_`` and dashes turned into ``_``
  (``double-click`` is ``desktop_double_click``, ``skills get`` is ``desktop_skills_get``);
* each argument and flag is a property of the input schema: its type, whether it is required, its enum, its default
  and its help text come from the help block;
* ``params`` records how each property reaches the command line, which ``build_argv`` follows.

Options that every command shares (the root command's ``Options:``) are the driver's, not the model's: logging, trace
files and session selection are never exposed, and the flags that change how an action is delivered are exposed only on
the commands they apply to. Everything here is pure, so it runs the same on any machine.
"""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any

PREFIX = "desktop_"
#: Root options the driver owns: they would let a model write files, rename the session or change logging.
DRIVER_ONLY = frozenset(
    {"verbose", "debug", "screenshot", "session", "agent-id", "trace", "trace-strict"}
)
#: Root options that are always the CLI's own, never a tool property.
BUILTIN = frozenset({"help", "version"})

_OPTION = re.compile(
    r"^ {2}(?:-\w, | {4})--(?P<long>[\w-]+)(?: +(?P<value>\[?<[^>]+>\]?))?(?: {2,}(?P<help>.*))?$"
)
_ARGUMENT = re.compile(
    r"^ {2}(?P<open>[<\[])(?P<name>[^>\]]+)[>\]](?P<more>\.\.\.)?(?: {2,}(?P<help>.*))?$"
)
_COMMAND = re.compile(r"^ {2}(?P<name>[a-z][a-z0-9-]*)(?: {2,}(?P<help>.*))?$")
_DEFAULT = re.compile(r"\s*\[default: (?P<v>[^\]]*)\]")
_POSSIBLE = re.compile(r"\s*\[possible values: (?P<v>[^\]]*)\]")
#: Value placeholders (and description hints) that name a whole number or a real number.
_INTEGER_VALUES = re.compile(
    r"^(?:MS|COUNT|LIMIT|NTH|PORT|WIDTH|HEIGHT|X|Y|DX|DY|AMOUNT|SCREEN|INDEX|"
    r"[A-Z_]*(?:TIMEOUT|TIMEOUT_MS|DEPTH|DURATION|WORDS|THAN))$"
)
_NUMBER_VALUES = frozenset({"N", "PX", "SIZE", "BOW"})


def snake(name: str) -> str:
    return name.replace("-", "_").lower()


def _section(lines: list[str], title: str) -> list[str]:
    """The indented lines under ``title:``, up to the next blank line."""
    for i, line in enumerate(lines):
        if line.rstrip() == f"{title}:":
            block: list[str] = []
            for follow in lines[i + 1 :]:
                if not follow.strip():
                    break
                block.append(follow.rstrip())
            return block
    return []


def _join_wrapped(block: list[str], start: re.Pattern[str]) -> list[tuple[re.Match[str], str]]:
    """Each entry of a block with the lines that continue its help text folded in."""
    entries: list[tuple[re.Match[str], list[str]]] = []
    for line in block:
        match = start.match(line)
        if match and not line.startswith(" " * 12):
            entries.append((match, [match.group("help") or ""]))
        elif entries:
            entries[-1][1].append(line.strip())
    return [(m, " ".join(p for p in parts if p).strip()) for m, parts in entries]


def parse_help(text: str) -> dict[str, Any]:
    """One command's help as ``summary``, ``subcommands``, ``arguments`` and ``options``."""
    lines = text.splitlines()
    usage = next((i for i, line in enumerate(lines) if line.startswith("Usage:")), len(lines))
    summary = " ".join(line.strip() for line in lines[:usage] if line.strip())
    subcommands = [
        (m.group("name"), h)
        for m, h in _join_wrapped(_section(lines, "Commands"), _COMMAND)
        if m.group("name") != "help"
    ]
    arguments = []
    for m, h in _join_wrapped(_section(lines, "Arguments"), _ARGUMENT):
        arguments.append(
            {
                "name": m.group("name"),
                "required": m.group("open") == "<",
                "variadic": bool(m.group("more")),
                "help": h,
            }
        )
    options = []
    for m, h in _join_wrapped(_section(lines, "Options"), _OPTION):
        options.append({"long": m.group("long"), "value": m.group("value"), "help": h})
    return {
        "summary": summary,
        "subcommands": subcommands,
        "arguments": arguments,
        "options": options,
    }


def _clean(help_text: str) -> tuple[str, str | None, list[str]]:
    """Help text without its ``[default: ...]`` and ``[possible values: ...]`` tails, and those two."""
    default = possible = None
    if m := _POSSIBLE.search(help_text):
        possible = [v.strip() for v in m.group("v").split(",") if v.strip()]
        help_text = _POSSIBLE.sub("", help_text)
    if m := _DEFAULT.search(help_text):
        default = m.group("v")
        help_text = _DEFAULT.sub("", help_text)
    return help_text.strip(), default, possible or []


def _kind_of(value: str | None, help_text: str, default: str | None) -> tuple[str, str]:
    """``(kind, type)``: how the property reaches argv, and its JSON type."""
    if value is None:
        return "bool", "boolean"
    optional = value.startswith("[")
    name = value.strip("[]<>")
    if re.search(r"\brepeatable\b", help_text, flags=re.IGNORECASE):
        return "multi", "string"
    json_type = "string"
    if default is not None and re.fullmatch(r"-?\d+", default):
        json_type = "integer"
    elif default is not None and re.fullmatch(r"-?\d+\.\d+", default):
        json_type = "number"
    elif name in _NUMBER_VALUES:
        json_type = "number"
    elif _INTEGER_VALUES.match(name):
        json_type = "integer"
    return ("optional_value" if optional else "value"), json_type


def _param(spec: dict[str, Any], *, positional: bool) -> dict[str, Any]:
    """One property: its name, argv mapping and schema entry."""
    help_text, default, possible = _clean(spec["help"])
    if positional:
        kind = "multi" if spec["variadic"] else "value"
        json_type = "integer" if _INTEGER_VALUES.match(spec["name"]) else "string"
        name, flag, required = snake(spec["name"]), None, spec["required"]
    else:
        kind, json_type = _kind_of(spec["value"], help_text, default)
        name, flag, required = snake(spec["long"]), f"--{spec['long']}", False
    schema: dict[str, Any] = {"type": json_type}
    if kind == "multi":
        schema = {"type": "array", "items": {"type": json_type}}
    elif kind == "optional_value":
        schema = {"type": [json_type, "boolean"]}
    if possible:
        schema["enum"] = possible
    if help_text:
        schema["description"] = help_text
    if default is not None and kind in ("value", "bool"):
        schema["default"] = int(default) if json_type == "integer" else default
        if json_type == "number":
            schema["default"] = float(default)
    return {
        "name": name,
        "flag": flag,
        "kind": kind,
        "required": required,
        "schema": schema,
    }


def _same_option(a: dict[str, Any], b: dict[str, Any]) -> bool:
    return (a["long"], a["value"], a["help"]) == (b["long"], b["value"], b["help"])


def _tool(words: list[str], parsed: dict[str, Any], shared: list[dict[str, Any]]) -> dict[str, Any]:
    has_ref = any(a["name"] == "REF" for a in parsed["arguments"])
    own = [o for o in parsed["options"] if not any(_same_option(s, o) for s in shared)]
    own_text = " ".join([parsed["summary"], *(o["help"] for o in own)])
    params = [_param(a, positional=True) for a in parsed["arguments"]]
    params += [_param(o, positional=False) for o in own]
    for option in parsed["options"]:
        if option in own or option["long"] in DRIVER_ONLY | BUILTIN:
            continue
        # A delivery or wait flag every command accepts: offered where the command acts on a ref or names it.
        if has_ref or f"--{option['long']}" in own_text:
            params.append(_param(option, positional=False))
    names = [p["name"] for p in params]
    if len(set(names)) != len(names):
        raise ValueError(f"{' '.join(words)}: two properties share a name: {sorted(names)}")
    return {
        "name": PREFIX + "_".join(snake(w) for w in words),
        "command": words,
        "description": parsed["summary"],
        "params": params,
        "inputSchema": schema_of(params),
    }


def schema_of(params: list[dict[str, Any]]) -> dict[str, Any]:
    schema: dict[str, Any] = {
        "type": "object",
        "properties": {p["name"]: p["schema"] for p in params},
        "additionalProperties": False,
    }
    if required := [p["name"] for p in params if p["required"]]:
        schema["required"] = required
    return schema


HelpFor = Callable[[list[str]], Awaitable[str]]


async def derive(help_for: HelpFor) -> list[dict[str, Any]]:
    """Every leaf command of the CLI as a tool, in the order the help lists them."""
    root = parse_help(await help_for([]))
    shared = root["options"]
    tools: list[dict[str, Any]] = []

    async def walk(words: list[str]) -> None:
        parsed = parse_help(await help_for(words))
        if parsed["subcommands"]:
            for name, _ in parsed["subcommands"]:
                await walk([*words, name])
        else:
            tools.append(_tool(words, parsed, shared))

    for name, _ in root["subcommands"]:
        await walk([name])
    names = [t["name"] for t in tools]
    if len(set(names)) != len(names):
        raise ValueError("two commands map to one tool name")
    return tools


def build_argv(tool: dict[str, Any], arguments: dict[str, Any]) -> list[str]:
    """The command line of one call: the command words, the flags as ``--flag=value``, then ``--`` and the
    positionals, so a value that starts with a dash is never taken for a flag. Raises ``ValueError`` for an argument
    the tool doesn't take, a missing required one, or a value of the wrong type."""
    params = {p["name"]: p for p in tool["params"]}
    if unknown := sorted(set(arguments) - set(params)):
        raise ValueError(f"{tool['name']} takes no argument named {', '.join(unknown)}")
    given = {k: v for k, v in arguments.items() if v is not None}
    if missing := [p["name"] for p in tool["params"] if p["required"] and p["name"] not in given]:
        raise ValueError(f"{tool['name']} needs {', '.join(missing)}")
    flags: list[str] = list(tool["command"])
    positionals: list[str] = []
    gap: str | None = None
    for p in tool["params"]:
        if p["name"] not in given:
            if p["flag"] is None:
                gap = gap or p["name"]
            continue
        value = given[p["name"]]
        _check_type(tool["name"], p, value)
        if p["flag"] is None:
            if gap:
                raise ValueError(f"{tool['name']} needs {gap} before {p['name']}")
            positionals.extend(str(v) for v in value) if isinstance(
                value, list
            ) else positionals.append(_text(value))
        elif p["kind"] == "bool":
            if value:
                flags.append(p["flag"])
        elif p["kind"] == "multi":
            flags.extend(f"{p['flag']}={_text(v)}" for v in value)
        elif p["kind"] == "optional_value" and value is True:
            flags.append(p["flag"])
        elif p["kind"] == "optional_value" and value is False:
            continue
        else:
            flags.append(f"{p['flag']}={_text(value)}")
    return [*flags, *(["--", *positionals] if positionals else [])]


def _text(value: Any) -> str:
    return "true" if value is True else "false" if value is False else str(value)


def _check_type(tool: str, p: dict[str, Any], value: Any) -> None:
    schema = p["schema"]
    kind = schema.get("type")
    if p["kind"] == "multi":
        ok = isinstance(value, list)
    elif kind == "boolean":
        ok = isinstance(value, bool)
    elif kind == "integer":
        ok = isinstance(value, int) and not isinstance(value, bool)
    elif kind == "number":
        ok = isinstance(value, (int, float)) and not isinstance(value, bool)
    elif isinstance(kind, list):  # an option whose value is optional: a value or a bare flag
        ok = isinstance(value, bool) or (
            isinstance(value, (int, float, str)) and not isinstance(value, bool)
        )
    else:
        ok = isinstance(value, (str, int, float)) and not isinstance(value, bool)
    if not ok:
        raise ValueError(f"{tool}: {p['name']} must be of type {kind}")
    if "enum" in schema and not isinstance(value, list) and str(value) not in schema["enum"]:
        raise ValueError(f"{tool}: {p['name']} must be one of {', '.join(schema['enum'])}")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def pinned_document(
    version: str, binary_sha256: str, tools: list[dict[str, Any]]
) -> dict[str, Any]:
    """What ``tools.json`` holds: the pin and the derived tools."""
    return {"version": version, "executable_sha256": binary_sha256, "tools": tools}


def diff_catalogs(pinned: list[dict[str, Any]], derived: list[dict[str, Any]]) -> list[str]:
    """What differs between two tool lists, as short lines (empty when they are equal)."""
    a, b = {t["name"]: t for t in pinned}, {t["name"]: t for t in derived}
    out = []
    if gone := sorted(set(a) - set(b)):
        out.append(f"missing from the binary: {', '.join(gone)}")
    if new := sorted(set(b) - set(a)):
        out.append(f"not in the pinned manifest: {', '.join(new)}")
    if changed := sorted(n for n in set(a) & set(b) if a[n] != b[n]):
        out.append(f"changed: {', '.join(changed)}")
    return out


def load_cache(folder: Path, binary_sha256: str) -> list[dict[str, Any]] | None:
    """A catalog derived earlier from this exact binary, or ``None`` when there is none or it can't be read."""
    try:
        data = json.loads((folder / f"catalog-{binary_sha256}.json").read_text())
        if data.get("executable_sha256") == binary_sha256 and isinstance(data.get("tools"), list):
            return data["tools"]
    except (OSError, ValueError, AttributeError):
        pass
    return None


def save_cache(folder: Path, binary_sha256: str, tools: list[dict[str, Any]]) -> None:
    """Best effort: a cache that can't be written costs one more derivation, never a failure."""
    try:
        folder.mkdir(parents=True, exist_ok=True)
        target = folder / f"catalog-{binary_sha256}.json"
        scratch = target.with_suffix(".tmp")
        scratch.write_text(json.dumps({"executable_sha256": binary_sha256, "tools": tools}))
        scratch.replace(target)
    except OSError:
        pass

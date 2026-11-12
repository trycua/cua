#!/usr/bin/env python3
"""Dump the `cb` (cua-bench) CLI and its task and result schemas as JSON.

Run inside the cua-bench project environment (the generator does this):

    uv run --frozen --python 3.12 --project libs/cua-bench \
        python scripts/docs-generators/extract_cb_docs.py

- ``cli``: the argparse tree of ``cua_bench.cli.main.build_parser()`` in the
  ``dump-docs --type cli`` schema the other CLIs emit (commands, arguments,
  options, flags, examples from each parser's epilog, exit codes).
- ``task``: the task module contract: ``cb.Task``, ``setup_config``
  (``DesktopSetupConfig``), the lifecycle decorators, the ``DesktopSession``
  methods and the action types.
- ``results``: ``result.json`` (``JobResult`` plus the Harbor fields) and
  ``summary.json`` (``Summary``), and the key sets the export goldens freeze
  (``cua_bench/tests/golden``) so the generator can check both agree.

Field descriptions are the ``#:`` comments above each field in the source.
Output is deterministic: no paths, dates or host facts.
"""

from __future__ import annotations

import argparse
import ast
import inspect
import json
import sys
from pathlib import Path

import cua_bench
from cua_bench.cli import main as cli_main

PKG = Path(cua_bench.__file__).resolve().parent


# ---------------------------------------------------------------- CLI

_TYPES = {"int": "integer", "float": "number", "str": "string", "Path": "path"}


def _type(action: argparse.Action) -> str:
    if action.choices:
        return " | ".join(str(c) for c in action.choices)
    t = getattr(action.type, "__name__", None) if action.type else None
    return _TYPES.get(t or "str", t or "string")


def _default(action: argparse.Action):
    d = action.default
    if d is None or d is argparse.SUPPRESS or isinstance(d, bool):
        return None
    if isinstance(d, (list, tuple)):
        return ",".join(str(x) for x in d) or None
    return str(d)


def _hidden(action: argparse.Action) -> bool:
    return action.help == argparse.SUPPRESS


def _help(action: argparse.Action) -> str:
    h = action.help or ""
    return "" if h is argparse.SUPPRESS else h.replace("%(default)s", str(action.default))


def _names(action: argparse.Action) -> tuple[str, str | None, list[str]]:
    longs = [o[2:] for o in action.option_strings if o.startswith("--")]
    shorts = [o[1:] for o in action.option_strings if not o.startswith("--")]
    name = longs[0] if longs else ""
    return name, (shorts[0] if shorts else None), longs[1:]


def _command(name: str, help_: str, parser: argparse.ArgumentParser) -> dict:
    arguments, options, flags, subcommands = [], [], [], []
    subcommand_optional = False
    for action in parser._actions:
        if isinstance(action, argparse._HelpAction):
            continue
        if isinstance(action, argparse._SubParsersAction):
            subcommand_optional = not action.required
            helps = {a.dest: a.help for a in action._choices_actions}
            seen: dict[int, dict] = {}
            for sub_name, sub in action.choices.items():
                if id(sub) in seen:
                    seen[id(sub)].setdefault("aliases", []).append(sub_name)
                    continue
                cmd = _command(sub_name, helps.get(sub_name) or "", sub)
                cmd["hidden"] = sub_name not in helps or helps.get(sub_name) is argparse.SUPPRESS
                seen[id(sub)] = cmd
                subcommands.append(cmd)
            continue
        if not action.option_strings:
            nargs = action.nargs
            arguments.append(
                {
                    "name": action.metavar or action.dest,
                    "help": _help(action),
                    "type": _type(action),
                    "possible_values": [str(c) for c in action.choices] if action.choices else [],
                    "default_value": _default(action) if nargs in ("?", "*") else None,
                    "is_optional": nargs in ("?", "*"),
                    "repeatable": nargs in ("*", "+"),
                    "hidden": _hidden(action),
                }
            )
            continue
        long, short, aliases = _names(action)
        base = {
            "name": long,
            "short_name": short,
            "aliases": aliases,
            "help": _help(action),
            "env": None,
            "hidden": _hidden(action),
        }
        if action.nargs == 0 or isinstance(action, argparse._CountAction):
            flags.append(
                {
                    **base,
                    "default_value": False,
                    "repeatable": isinstance(action, argparse._CountAction),
                    "takes_value": False,
                }
            )
        else:
            options.append(
                {
                    **base,
                    "type": _type(action),
                    "value_name": action.metavar if isinstance(action.metavar, str) else None,
                    "possible_values": [str(c) for c in action.choices] if action.choices else [],
                    "default_value": _default(action),
                    "is_optional": not action.required,
                    "repeatable": isinstance(action, argparse._AppendAction),
                    "multiple_values": action.nargs in ("+", "*") or (isinstance(action.nargs, int) and action.nargs > 1),
                    "takes_value": True,
                }
            )
    cmd = {
        "name": name,
        "abstract": help_ if help_ is not argparse.SUPPRESS else "",
        "arguments": arguments,
        "options": options,
        "flags": flags,
        "subcommands": subcommands,
    }
    if subcommands:
        cmd["subcommand_optional"] = subcommand_optional
    if parser.description and parser.description.strip() != (help_ or "").strip():
        cmd["discussion"] = parser.description.strip()
    if parser.epilog:
        cmd["after_help"] = parser.epilog.strip()
    return cmd


def cli() -> dict:
    parser = cli_main.build_parser()
    root = _command("cb", parser.description or "", parser)
    return {
        "name": "cb",
        "version": cli_main._get_version(),
        "abstract": parser.description or "",
        "usage": "cb <COMMAND> [OPTIONS]",
        "global_options": [],
        "exit_codes": [{"code": c, "meaning": m} for c, m in cli_main.EXIT_CODES],
        "after_help": root.get("after_help"),
        "commands": root["subcommands"],
    }


# ---------------------------------------------------------------- schemas


def _doc_comments(lines: list[str], lineno: int) -> str:
    """The ``#:`` comment block right above 1-based ``lineno``."""
    out: list[str] = []
    i = lineno - 2
    while i >= 0 and lines[i].strip().startswith("#:"):
        out.insert(0, lines[i].strip()[2:].strip())
        i -= 1
    return " ".join(out)


def _simplify(annotation: str) -> str:
    a = annotation.replace("typing.", "")
    if a.startswith("Optional[") and a.endswith("]"):
        return f"{_simplify(a[9:-1])} | null"
    if a.startswith("Literal["):
        return "string"
    if a.startswith("List[") and a.endswith("]"):
        return f"list[{_simplify(a[5:-1])}]"
    if a == "Any":
        return "any"
    if a == "None":
        return "null"
    return a


def class_fields(path: Path, name: str) -> dict:
    """Fields of the class ``name`` in ``path``: type, default, ``#:`` doc."""
    source = path.read_text()
    lines = source.splitlines()
    tree = ast.parse(source)
    cls = next(n for n in ast.walk(tree) if isinstance(n, ast.ClassDef) and n.name == name)
    fields = []
    for node in cls.body:
        if not isinstance(node, ast.AnnAssign) or not isinstance(node.target, ast.Name):
            continue
        default = None
        if node.value is not None:
            default = ast.unparse(node.value)
            if default.startswith("field(default_factory="):
                factory = default[len("field(default_factory=") : -1]
                default = {"dict": "{}", "list": "[]"}.get(factory, factory)
        fields.append(
            {
                "name": node.target.id,
                "type": _simplify(ast.unparse(node.annotation)),
                "default": default,
                "description": _doc_comments(lines, node.lineno),
            }
        )
    return {"name": name, "doc": inspect.cleandoc(ast.get_docstring(cls) or ""), "fields": fields}


def _summary(doc: str | None) -> str:
    return inspect.cleandoc(doc or "").split("\n\n")[0].replace("\n", " ").strip()


def session_methods() -> dict:
    """The public API of the session a task receives (``RemoteDesktopSession``,
    the only ``DesktopSession`` implementation)."""
    path = PKG / "computers" / "remote.py"
    tree = ast.parse(path.read_text())
    cls = next(
        n
        for n in ast.walk(tree)
        if isinstance(n, ast.ClassDef) and n.name == "RemoteDesktopSession"
    )
    methods = []
    for node in cls.body:
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if node.name.startswith("_"):
            continue
        decorators = {ast.unparse(d) for d in node.decorator_list}
        is_property = "property" in decorators
        if "classmethod" in decorators or "staticmethod" in decorators:
            continue
        args = ast.unparse(node.args)
        args = args[len("self") :].lstrip(", ") if args.startswith("self") else args
        returns = ast.unparse(node.returns) if node.returns else ("Any" if is_property else "None")
        methods.append(
            {
                "name": node.name,
                "kind": "property" if is_property else "method",
                "is_async": isinstance(node, ast.AsyncFunctionDef),
                "signature": node.name if is_property else f"{node.name}({args})",
                "returns": returns.strip("'\""),
                "summary": _summary(ast.get_docstring(node)),
            }
        )
    return {"doc": _summary(ast.get_docstring(cls)), "methods": methods}


def decorators() -> list[dict]:
    from cua_bench import decorators as d

    return [
        {"name": name, "summary": _summary(getattr(d, name).__doc__), "doc": inspect.cleandoc(getattr(d, name).__doc__ or "")}
        for name in ("tasks_config", "setup_task", "solve_task", "evaluate_task")
    ]


def actions() -> list[dict]:
    from cua_bench import types

    names = [n for n in types.__all__ if n.endswith("Action") and n != "Action"]
    path = PKG / "types.py"
    return [class_fields(path, n) for n in names]


def results() -> dict:
    golden = PKG / "tests" / "golden"
    return {
        "job_result": class_fields(PKG / "runner" / "batch_runner.py", "JobResult"),
        "harbor": class_fields(PKG / "results.py", "HarborTrialFields"),
        "span": class_fields(PKG / "results.py", "Span"),
        "summary": class_fields(PKG / "results.py", "Summary"),
        "summary_result": class_fields(PKG / "results.py", "SummaryResult"),
        "summary_stats": class_fields(PKG / "results.py", "SummaryStats"),
        "summary_target": class_fields(PKG / "results.py", "SummaryTarget"),
        "golden_result": json.loads((golden / "result_schema.json").read_text()),
        "golden_summary": json.loads((golden / "summary_schema.json").read_text()),
    }


def main() -> int:
    out = {
        "cli": cli(),
        "task": {
            "task": class_fields(PKG / "core.py", "Task"),
            "setup_config": class_fields(PKG / "computers" / "base.py", "DesktopSetupConfig"),
            "decorators": decorators(),
            "session": session_methods(),
            "actions": actions(),
        },
        "results": results(),
    }
    json.dump(out, sys.stdout, indent=1, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

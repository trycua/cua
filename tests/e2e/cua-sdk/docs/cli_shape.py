#!/usr/bin/env python3
"""cli-shape lane: every ``cua``, ``cua-driver``, ``lume`` and ``cb`` command in a
docs block tagged ``test="cli-shape"`` must parse against that CLI's
generated definition (``scripts/docs-generators/cli-specs/<cli>.json``, the
same ``dump-docs --type cli`` JSON the reference pages are rendered from).

Nothing runs: commands are split with the stdlib ``shlex`` (never bashlex,
which is GPL) plus a pipeline / ``&&`` / ``;`` splitter, then the
subcommand path, options, option values and positional counts are checked.
Placeholders such as ``<name>`` count as values.

    cli_shape.py                       # check every cli-shape block
    cli_shape.py --suggest             # which baselined blocks would pass
    cli_shape.py --tag                 # tag those blocks test="cli-shape"

With ``$CUA_E2E_RESULTS`` set, results go to docs-blocks.jsonl (lane
``cli-shape``) for docs/coverage.py. Stdlib only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import sys
from dataclasses import dataclass
from pathlib import Path

import extract

SPECS = extract.REPO / "scripts" / "docs-generators" / "cli-specs"
CLIS = ("cua", "cua-driver", "lume", "cb")
SHELL_LANGS = {"bash", "console", "powershell"}
LANE = "cli-shape"
# `<name>`, `<sandbox-name>`, `<ref|path>`: a value the reader fills in.
PLACEHOLDER = re.compile(r"<[A-Za-z][\w .:/|@=-]*>")
PLACEHOLDER_TOKEN = "__PLACEHOLDER__"
OPERATORS = {"|", "||", "&&", ";", "&", "(", ")", "|&", ";;"}
REDIRECTS = {">", ">>", "<", "<<", "<<<", ">&", "&>", "&>>", ">|", "<>"}
PREFIXES = {"sudo", "env", "exec", "time", "command", "nohup", "&"}
HELP = {"-h", "--help", "help"}
VERSION = {"-v", "-V", "--version", "version"}
# cua-driver runs any registered MCP tool by name: `cua-driver <tool> [json]`.
DRIVER_TOOL = re.compile(r"^[a-z][a-z0-9]*(_[a-z0-9]+)+$")


def load_specs(root: Path = SPECS) -> dict[str, dict]:
    return {
        cli: json.loads((root / f"{cli}.json").read_text(encoding="utf-8"))
        for cli in CLIS
        if (root / f"{cli}.json").exists()
    }


# ---------------------------------------------------------------- shell split


def logical_lines(code: str, lang: str) -> list[str]:
    """Commands of a block: continuations joined, prompts and output lines of
    console blocks dropped, comments and heredoc bodies skipped."""
    lang = extract.norm_lang(lang)
    raw = code.splitlines()
    if lang == "console":
        # Only prompted lines are commands; the rest is output.
        prompted = []
        for ln in raw:
            s = ln.lstrip()
            if s.startswith("$ ") or s.startswith("% ") or s.startswith("> "):
                prompted.append(s[2:])
            elif prompted and prompted[-1].rstrip().endswith("\\"):
                prompted.append(ln)
        raw = prompted
    cont = "`" if lang == "powershell" else "\\"
    out: list[str] = []
    buf = ""
    heredoc: str | None = None
    for ln in raw:
        if heredoc is not None:
            if ln.strip() == heredoc:
                heredoc = None
            continue
        s = ln.rstrip()
        if lang != "console" and s.lstrip().startswith("$ "):
            s = s.lstrip()[2:]
        if s.endswith(cont):
            buf += s[:-1] + " "
            continue
        line = (buf + s).strip()
        buf = ""
        m = re.search(r"<<-?\s*['\"]?([A-Za-z_]\w*)['\"]?", line)
        if m:
            heredoc = m[1]
            line = line[: m.start()]
        if line and not line.startswith("#"):
            out.append(line)
    if buf.strip():
        out.append(buf.strip())
    return out


def simple_commands(line: str) -> list[list[str]]:
    """``a | b && c; d`` -> [[a...], [b...], [c...], [d...]] (redirections dropped)."""
    line = PLACEHOLDER.sub(PLACEHOLDER_TOKEN, line)
    lex = shlex.shlex(line, posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    lex.commenters = "#"
    try:
        tokens = list(lex)
    except ValueError as e:
        raise ShapeError(f"cannot tokenize: {e}") from e
    cmds: list[list[str]] = [[]]
    skip_next = False
    for tok in tokens:
        if skip_next:
            skip_next = False
            continue
        if tok in OPERATORS:
            cmds.append([])
            continue
        if tok in REDIRECTS or re.fullmatch(r"\d?[<>]+&?\d*", tok):
            skip_next = not tok.endswith(("&1", "&2"))
            continue
        if tok == "$":  # the `$` of `$(...)`; `(` already split the command
            continue
        cmds[-1].append(tok)
    return [c for c in cmds if c]


def invocation(cmd: list[str]) -> tuple[str, list[str]] | None:
    """(cli, args) when this simple command runs cua, cua-driver, lume or cb."""
    i = 0
    while i < len(cmd) and (re.match(r"^[A-Za-z_]\w*=", cmd[i]) or cmd[i] in PREFIXES):
        i += 1
    if i >= len(cmd):
        return None
    word = cmd[i].replace("\\", "/").rsplit("/", 1)[-1].removesuffix(".exe")
    if word in CLIS:
        return word, cmd[i + 1 :]
    return None


# ---------------------------------------------------------------- validation


class ShapeError(Exception):
    pass


@dataclass
class Scope:
    options: dict[str, dict]  # "--name" / "-s" -> option spec


def _names(opt: dict) -> list[str]:
    names = []
    if opt.get("name"):
        names.append(f"--{opt['name']}")
    if opt.get("short_name"):
        names.append(f"-{opt['short_name']}")
    for a in opt.get("aliases") or []:
        names.append(a if a.startswith("-") else f"--{a}")
    return names


def _scope(cmd: dict) -> dict[str, dict]:
    out = {}
    for opt in cmd.get("options", []):
        opt = dict(opt, takes_value=opt.get("takes_value", True))
        for n in _names(opt):
            out[n] = opt
    for flag in cmd.get("flags", []):
        flag = dict(flag, takes_value=False)
        for n in _names(flag):
            out[n] = flag
    return out


def _choices(opt: dict) -> list[str] | None:
    if opt.get("possible_values"):
        return [str(v) for v in opt["possible_values"]]
    t = opt.get("type") or ""
    if " | " in t:
        return [x.strip() for x in t.split(" | ")]
    return None


def _check_value(opt: dict, value: str, where: str) -> None:
    choices = _choices(opt)
    if choices and PLACEHOLDER_TOKEN not in value and not value.startswith("$"):
        if value not in choices:
            raise ShapeError(f"{where}: --{opt['name']} takes {' | '.join(choices)}, not {value!r}")


# `cb run <path>` is shorthand for `cb run task|dataset <path>` (cua_bench.cli.main.normalize_argv).
CB_RUN_SUBCOMMANDS = {"task", "dataset", "list", "info", "watch", "stop", "logs"}


def validate(spec: dict, args: list[str], cli: str) -> None:
    """Raises ShapeError when ``args`` do not parse against ``spec``."""
    if (
        cli == "cb"
        and len(args) >= 2
        and args[0] == "run"
        and args[1] not in CB_RUN_SUBCOMMANDS
        and not args[1].startswith("-")
    ):
        errors = []
        for kind in ("task", "dataset"):
            try:
                return _validate(spec, ["run", kind, *args[1:]], cli)
            except ShapeError as e:
                errors.append(str(e))
        raise ShapeError(errors[0])
    _validate(spec, args, cli)


def _validate(spec: dict, args: list[str], cli: str) -> None:
    globals_ = _scope({"options": spec.get("global_options", []), "flags": []})
    node: dict = {"subcommands": spec["commands"], "arguments": [], "options": [], "flags": []}
    path = [cli]
    scope = dict(globals_)
    positionals: list[str] = []
    after_dashes: list[str] = []
    trailing = False
    i = 0
    while i < len(args):
        tok = args[i]
        where = " ".join(path)
        if trailing:
            after_dashes.append(tok)
        elif _in_vararg(node, positionals):
            positionals.append(tok)
        elif tok == "--":
            trailing = True
        elif tok in HELP and tok.startswith("-") or (tok == "help" and node.get("subcommands")):
            return
        elif tok in VERSION and len(path) == 1:
            return
        elif tok.startswith("-") and len(tok) > 1 and not re.fullmatch(r"-\d+(\.\d+)?", tok):
            name, eq, value = tok.partition("=")
            opt = scope.get(name)
            if opt is None and not name.startswith("--") and len(name) > 2:
                # A cluster of short flags (-it) or a short option with its value (-n5).
                opt = scope.get(name[:2])
                if opt is not None and opt.get("takes_value"):
                    eq, value = "=", name[2:]
                elif opt is not None:
                    for ch in name[2:]:
                        if f"-{ch}" not in scope:
                            raise ShapeError(f"{where}: unknown option -{ch} (in {tok})")
            if opt is None:
                raise ShapeError(f"{where}: unknown option {name}")
            if opt.get("takes_value"):
                if not eq:
                    i += 1
                    if i >= len(args):
                        raise ShapeError(f"{where}: {name} needs a value")
                    value = args[i]
                _check_value(opt, value, where)
                # `--tags a b c`: one occurrence takes values up to the next option.
                while (
                    opt.get("multiple_values")
                    and i + 1 < len(args)
                    and not args[i + 1].startswith("-")
                    and args[i + 1] not in OPERATORS
                ):
                    i += 1
                    _check_value(opt, args[i], where)
            elif eq:
                raise ShapeError(f"{where}: {name} takes no value")
        elif node.get("subcommands") and (
            len(positionals) >= len(node.get("arguments", []))
            or (not positionals and _find(node["subcommands"], tok) is not None)
        ):
            sub = _find(node["subcommands"], tok)
            if sub is None:
                if cli == "cua-driver" and len(path) == 1 and DRIVER_TOOL.match(tok):
                    return  # `cua-driver <tool> [json]`: the tool registry decides
                names = sorted(c["name"] for c in node["subcommands"] if not c.get("hidden"))
                raise ShapeError(
                    f"{where}: unknown command {tok!r} (expected one of {', '.join(names)})"
                )
            node = sub
            positionals = []
            path.append(sub["name"])
            scope = dict(globals_) | _scope(sub)
        else:
            positionals.append(tok)
        i += 1
    where = " ".join(path)
    if node.get("subcommands") and len(path) == 1:
        raise ShapeError(f"{where}: missing command")
    if node.get("subcommands") and not node.get("arguments") and not _is_leaf_ok(node):
        raise ShapeError(f"{where}: missing subcommand")
    spec_args = [a for a in node.get("arguments", [])]
    if _last_after_dashes(node):
        # `[-- <COMMAND>...]`: that argument only takes what follows `--`.
        spec_args = spec_args[:-1]
    else:
        positionals += after_dashes
    required = [a for a in spec_args if not a.get("is_optional")]
    repeatable = any(a.get("repeatable") for a in spec_args)
    if len(positionals) < len(required):
        missing = ", ".join(f"<{a['name']}>" for a in required[len(positionals) :])
        raise ShapeError(f"{where}: missing {missing}")
    if not repeatable and len(positionals) > len(spec_args):
        extra = positionals[len(spec_args) :]
        raise ShapeError(f"{where}: unexpected argument(s) {' '.join(extra)}")


def _last_after_dashes(node: dict) -> bool:
    usage = node.get("usage") or ""
    return bool(node.get("arguments")) and ("[-- <" in usage or " -- <" in usage)


def _in_vararg(node: dict, positionals: list[str]) -> bool:
    """Inside a trailing `<COMMAND>...` (clap trailing_var_arg): the words
    after the fixed positionals belong to the command, dashes included. A
    vararg that must follow `--` (usage `[-- <COMMAND>...]`) is not one."""
    args = node.get("arguments", [])
    if not args or not args[-1].get("repeatable") or node.get("subcommands"):
        return False
    if _last_after_dashes(node):
        return False
    return len(positionals) >= max(1, len(args) - 1)


def _is_leaf_ok(node: dict) -> bool:
    """clap subcommand groups with `args_conflicts_with_subcommands` or a
    default action accept no subcommand; the dump marks those with usage
    lacking `<COMMAND>`."""
    if node.get("default_subcommand") or node.get("subcommand_optional"):
        return True
    usage = node.get("usage") or ""
    return bool(usage) and "<COMMAND>" not in usage


def _find(cmds: list[dict], name: str) -> dict | None:
    for c in cmds:
        if (
            name == c["name"]
            or name in (c.get("aliases") or [])
            or name in (c.get("hidden_aliases") or [])
        ):
            return c
    return None


def check_block(code: str, lang: str, specs: dict[str, dict]) -> tuple[int, list[str]]:
    """(number of cua/cua-driver/lume/cb invocations, problems)."""
    n, problems = 0, []
    for line in logical_lines(code, lang):
        try:
            cmds = simple_commands(line)
        except ShapeError as e:
            problems.append(f"{line!r}: {e}")
            continue
        for cmd in cmds:
            inv = invocation(cmd)
            if inv is None:
                continue
            cli, args = inv
            if cli not in specs:
                problems.append(f"{line!r}: no CLI spec for {cli}")
                continue
            n += 1
            try:
                validate(specs[cli], args, cli)
            except ShapeError as e:
                problems.append(f"{line.strip()!r}: {e}")
    return n, problems


# ---------------------------------------------------------------- main


def _record(rows: list[dict]) -> None:
    out = os.environ.get("CUA_E2E_RESULTS")
    if not out:
        return
    Path(out).mkdir(parents=True, exist_ok=True)
    with open(Path(out) / "docs-blocks.jsonl", "a", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")


def _block_id(f: extract.Fence, taken: set[str], specs: dict[str, dict]) -> str:
    words = []
    for line in logical_lines(f.code, f.lang):
        for cmd in simple_commands(line):
            inv = invocation(cmd)
            if inv:
                words = [inv[0]] + [a for a in inv[1][:3] if re.fullmatch(r"[a-z][a-z0-9-]*", a)]
                break
        if words:
            break
    base = "-".join(words)[:40].strip("-") or "shell"
    bid, n = base, 2
    while bid in taken:
        bid, n = f"{base}-{n}", n + 1
    taken.add(bid)
    return bid


def tag(docs: Path, specs: dict[str, dict], baseline: set[tuple[str, str]]) -> int:
    """Adds test="cli-shape" id=... to baselined shell blocks whose every
    cua / cua-driver / lume invocation validates (and that have one)."""
    count = 0
    for path in sorted(docs.rglob("*.mdx")):
        rel = path.relative_to(docs).as_posix()
        fences = extract.fences(path, rel)
        taken = {f.block_id for f in fences if f.block_id}
        todo = {}
        for f in fences:
            if f.generated or f.disposition or (f.page, f.fingerprint) not in baseline:
                continue
            if extract.norm_lang(f.lang) not in SHELL_LANGS:
                continue
            n, problems = check_block(f.code, f.lang, specs)
            if n and not problems:
                todo[f.line] = _block_id(f, taken, specs)
        if not todo:
            continue
        lines = path.read_text(encoding="utf-8").split("\n")
        for line_no, bid in todo.items():
            lines[line_no - 1] = lines[line_no - 1].rstrip() + f' test="{LANE}" id="{bid}"'
            count += 1
        path.write_text("\n".join(lines), encoding="utf-8")
    return count


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--docs", type=Path, default=extract.DOCS)
    ap.add_argument("--specs", type=Path, default=SPECS)
    ap.add_argument("--suggest", action="store_true", help="report baselined shell blocks")
    ap.add_argument("--tag", action="store_true", help="tag baselined blocks that validate")
    a = ap.parse_args()
    specs = load_specs(a.specs)
    missing = [c for c in CLIS if c not in specs]
    if missing:
        print(f"missing CLI specs: {missing} in {a.specs}", file=sys.stderr)
        return 2
    if a.tag:
        print(f"tagged {tag(a.docs, specs, extract.load_baseline())} block(s)")
        return 0
    if a.suggest:
        baseline = extract.load_baseline()
        ok = bad = 0
        for f in extract.all_fences(a.docs):
            if f.disposition or (f.page, f.fingerprint) not in baseline:
                continue
            if extract.norm_lang(f.lang) not in SHELL_LANGS:
                continue
            n, problems = check_block(f.code, f.lang, specs)
            if not n:
                continue
            if problems:
                bad += 1
                for p in problems:
                    print(f"{f.page}:{f.line}: {p}")
            else:
                ok += 1
        print(f"\n{ok} baselined block(s) validate; {bad} have problems", file=sys.stderr)
        return 0
    rows, failures = [], []
    blocks = [b for b in extract.all_blocks(a.docs) if LANE in b.lanes]
    for b in blocks:
        n, problems = check_block(b.code, b.lang, specs)
        if not n:
            problems = ["no cua, cua-driver, lume or cb command to check"]
        status = "fail" if problems else "pass"
        rows.append(
            {
                "block_id": b.id,
                "page": b.guide,
                "line": b.line,
                "lane": LANE,
                "lang": b.lang,
                "status": status,
                "test": "cli_shape",
                "reason": "; ".join(problems)[:400],
            }
        )
        failures += [f"{b.guide}:{b.line} ({b.id}): {p}" for p in problems]
    _record(rows)
    if failures:
        print("cli-shape:", *failures, sep="\n  ", file=sys.stderr)
        return 1
    print(f"cli-shape OK: {len(blocks)} block(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

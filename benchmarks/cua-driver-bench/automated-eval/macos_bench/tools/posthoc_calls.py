#!/usr/bin/env python3
"""Post-hoc analyses of the tool calls in a run (labelled post-hoc in the report; not pre-registered).

  failed   every failed MCP call: tool, input, error text, what followed, whether a retry of the same tool worked
  actions  actions performed per trial and arm (Codex `js` calls are parsed into the actions they contain),
           observe calls per action, and the call mix
  posthoc_calls.py failed|actions RUN_DIR [--task CDB-S01 ...] [--arm cc-cua-driver]
"""

from __future__ import annotations

import argparse
import collections
import json
import re
import sys
from pathlib import Path
from typing import Any

OBSERVE_CUA = {"get_window_state", "get_desktop_state", "screenshot", "zoom", "list_windows", "list_apps", "get_accessibility_tree", "verify_state", "get_window_info", "get_screen_size", "get_cursor_position"}
# Codex cua_repl API: reads versus actions (names of cua.* and app/tab methods seen in the tool description)
CODEX_OBSERVE = re.compile(r"\.(getState|getApp|getTab|getAXState|getAXTree|getUiTree|screenshot|getScreenshot|listApps|getText|readText)\s*\(")
CODEX_ACTION = re.compile(r"\.(click|doubleClick|rightClick|type|typeText|press|pressKey|key|hotkey|scroll|drag|setValue|paste|selectText|moveMouse|hover|mouseDown|mouseUp|focus|openMenu|menu|performAction|select|clear|fill|navigate|goto|open|launch)\s*\(")


def events(path: Path) -> list[dict[str, Any]]:
    out = []
    for line in path.read_text("utf-8", "replace").splitlines():
        part = line.split("\t", 1)[-1]
        try:
            out.append(json.loads(part))
        except json.JSONDecodeError:
            pass
    return out


def calls(evs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Ordered tool calls with their results."""
    uses: dict[str, dict[str, Any]] = {}
    order: list[str] = []
    for e in evs:
        if e.get("type") == "assistant":
            for b in (e.get("message") or {}).get("content") or []:
                if isinstance(b, dict) and b.get("type") == "tool_use":
                    uses[b["id"]] = {"id": b["id"], "name": b["name"], "input": b.get("input", {}), "result": None, "error": False}
                    order.append(b["id"])
        elif e.get("type") == "user":
            content = (e.get("message") or {}).get("content")
            for b in content if isinstance(content, list) else []:
                if isinstance(b, dict) and b.get("type") == "tool_result" and b.get("tool_use_id") in uses:
                    c = b.get("content")
                    if isinstance(c, list):
                        c = " ".join(x.get("text", "") for x in c if isinstance(x, dict) and x.get("type") == "text")
                    uses[b["tool_use_id"]]["result"] = str(c or "")
                    uses[b["tool_use_id"]]["error"] = bool(b.get("is_error"))
    return [uses[i] for i in order]


def short(obj: Any, n: int = 200) -> str:
    return json.dumps(obj, ensure_ascii=False)[:n]


def trials(run: Path, tasks: list[str] | None, arm: str | None):
    for tdir in sorted((run / "trials").iterdir()):
        row = tdir / "a1" / "trial.json"
        if not row.exists():
            attempts = sorted(tdir.glob("a*/trial.json"))
            if not attempts:
                continue
            row = attempts[-1]
        meta = json.loads(row.read_text("utf-8"))
        if meta.get("smoke") or (tasks and meta["task"] not in tasks) or (arm and meta["arm"] != arm):
            continue
        yield meta, row.parent / "claude-stream.tsv"


def cmd_failed(run: Path, tasks, arm) -> None:
    for meta, stream in trials(run, tasks, arm):
        cs = calls(events(stream))
        for i, c in enumerate(cs):
            if not c["name"].startswith("mcp__") or not c["error"]:
                continue
            nxt = [x for x in cs[i + 1 : i + 4]]
            retry = next((x for x in nxt if x["name"] == c["name"]), None)
            print(f"{meta['trial_id']} #{i} {c['name']} input={short(c['input'])}")
            print(f"    error: {(c['result'] or '')[:300]!r}")
            print("    next: " + ", ".join(f"{x['name'].split('__')[-1]}{'(ERR)' if x['error'] else ''}" for x in nxt))
            if retry:
                print(f"    same-tool retry: {'failed' if retry['error'] else 'ok'} input={short(retry['input'], 140)}")


def codex_actions(code: str) -> tuple[int, int]:
    return len(CODEX_ACTION.findall(code)), len(CODEX_OBSERVE.findall(code))


def cmd_actions(run: Path, tasks, arm) -> None:
    rows = collections.defaultdict(list)
    for meta, stream in trials(run, tasks, arm):
        cs = [c for c in calls(events(stream)) if c["name"].startswith("mcp__")]
        a = o = 0
        for c in cs:
            short_name = c["name"].split("__", 2)[-1]
            if "codex-cu" in c["name"]:
                if short_name == "js":
                    act, obs = codex_actions(str(c["input"].get("code", "")))
                    a += act
                    o += obs
            else:
                if short_name in OBSERVE_CUA:
                    o += 1
                else:
                    a += 1
        rows[(meta["task"], meta["arm"])].append((len(cs), a, o))
    print("task, arm, trials, mean_mcp_calls, mean_actions, mean_observes, observes_per_action")
    for (task, a), v in sorted(rows.items()):
        n = len(v)
        mc, ma, mo = (sum(x[i] for x in v) / n for i in range(3))
        print(f"{task}, {a}, {n}, {mc:.1f}, {ma:.1f}, {mo:.1f}, {(mo / ma if ma else float('nan')):.2f}")
    allv = collections.defaultdict(list)
    for (task, a), v in rows.items():
        allv[a].extend(v)
    for a, v in sorted(allv.items()):
        n = len(v)
        mc, ma, mo = (sum(x[i] for x in v) / n for i in range(3))
        print(f"ALL, {a}, {n}, {mc:.1f}, {ma:.1f}, {mo:.1f}, {(mo / ma if ma else float('nan')):.2f}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("what", choices=["failed", "actions"])
    ap.add_argument("run", type=Path)
    ap.add_argument("--task", nargs="*")
    ap.add_argument("--arm")
    args = ap.parse_args()
    {"failed": cmd_failed, "actions": cmd_actions}[args.what](args.run, args.task, args.arm)
    return 0


if __name__ == "__main__":
    sys.exit(main())

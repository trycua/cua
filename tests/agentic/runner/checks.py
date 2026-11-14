"""The hard checks. Only these decide pass or fail.

relay       the relay API as the canary account sees it, with a jq predicate
cli         `cua ... --json` on host N, with a jq predicate
ui          an accessibility-tree assertion on the app's window, read through
            Cua Driver (contains / absent / row), optionally held for hold_s or
            reached within within_s, optionally timed from an event
no_dialogs  window titles that must never appear during the whole run
file        a file on host N exists, contains or lacks a text
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path

from . import canary
from .mcp import Mcp, tree_text, window_title, windows
from .provision import Machine, is_app_window
from .spec import render

DEFAULT_WITHIN_S = 15


@dataclass
class CheckResult:
    kind: str
    name: str
    ok: bool
    evidence: str
    skipped: bool = False


def jq_ok(expr: str, doc) -> tuple[bool, str]:
    jq = shutil.which("jq")
    if not jq:
        raise RuntimeError("jq is not installed")
    p = subprocess.run(
        [jq, "-e", expr], input=json.dumps(doc), capture_output=True, text=True, timeout=30
    )
    out = (p.stdout.strip() or p.stderr.strip())[:300]
    return p.returncode == 0, out


def first_json(text: str):
    text = text.strip()
    if not text:
        raise ValueError("no output")
    obj, _ = json.JSONDecoder().raw_decode(text)
    return obj


def _poll(fn, within_s: float, interval: float = 3.0):
    deadline = time.monotonic() + within_s
    while True:
        ok, ev = fn()
        if ok or time.monotonic() >= deadline:
            return ok, ev
        time.sleep(interval)


def app_tree(mcp: Mcp) -> tuple[str | None, str]:
    wins = [w for w in windows(mcp) if is_app_window(w)]
    if not wins:
        return None, "no Cua Spaces window"
    w = max(
        wins,
        key=lambda w: (w.get("bounds") or {}).get("width", 0)
        * (w.get("bounds") or {}).get("height", 0),
    )
    r = mcp.call(
        "get_window_state",
        {
            "pid": w["pid"],
            "window_id": w["window_id"],
            "max_elements": 2000,
            "include_screenshot": False,
            "timeout_ms": 10000,
        },
        timeout=90,
    )
    if r.is_error:
        return None, "get_window_state failed: " + r.first_line()
    return tree_text(r), f"window {window_title(w)!r}"


def keep_tree(m, name: str, tree: str) -> None:
    """The last tree a ui check read, as evidence beside the report."""
    d = m.local_dir / "ui-trees"
    d.mkdir(parents=True, exist_ok=True)
    slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")[:60] or "ui"
    (d / f"{slug}.md").write_text(tree, encoding="utf-8")


def ui_assert(tree: str, body: dict) -> tuple[bool, str]:
    low = tree.lower()
    for s in _list(body.get("contains")):
        if s.lower() not in low:
            return False, f"missing {s!r}"
    for s in [a for a in _list(body.get("absent")) if a.strip()]:
        if s.lower() in low:
            line = next((ln.strip() for ln in tree.splitlines() if s.lower() in ln.lower()), "")
            return False, f"present {s!r}: {line[:160]}"
    if "row" in body:
        # A row is the element whose line matches `has`, plus its own
        # subtree (the lines indented under it), never its neighbours.
        has, near = _list(body["row"]["has"]), _list(body["row"].get("and"))
        lines = tree.splitlines()
        for i, line in enumerate(lines):
            if all(h.lower() in line.lower() for h in has):
                depth = len(line) - len(line.lstrip())
                sub = [line]
                for nxt in lines[i + 1 :]:
                    if nxt.strip() and len(nxt) - len(nxt.lstrip()) <= depth:
                        break
                    sub.append(nxt)
                text = " ".join(sub).lower()
                missing = [a for a in near if a.lower() not in text]
                if not missing:
                    return True, f"row {line.strip()[:120]!r} has {near}"
                return (
                    False,
                    f"row {line.strip()[:120]!r} (and its {len(sub) - 1} children) lacks {missing}",
                )
        return False, f"no row with {has}"
    return True, "assertion holds"


def _list(v) -> list[str]:
    if v is None:
        return []
    return [v] if isinstance(v, str) else list(v)


class DialogWatcher:
    """Every window title seen on the machine from setup to the last check."""

    def __init__(self, machine: Machine, titles: list[str]):
        self.m = machine
        self.titles = [t.lower() for t in titles]
        self.seen: dict[str, float] = {}
        self.t0 = time.time()
        self._stop = threading.Event()
        self._mcp: Mcp | None = None
        self.thread = threading.Thread(target=self._run, daemon=True)

    def start(self):
        self._mcp = self.m.open_mcp("dialogs")
        self.thread.start()

    def _run(self):
        while not self._stop.is_set():
            try:
                for w in windows(self._mcp):
                    t = window_title(w)
                    if t and t not in self.seen:
                        self.seen[t] = round(time.time() - self.t0, 1)
            except Exception:
                pass
            self._stop.wait(2)

    def stop(self):
        self._stop.set()
        self.thread.join(timeout=10)
        if self._mcp:
            self._mcp.close()

    def matches(self, titles: list[str]) -> list[tuple[str, float]]:
        wanted = [t.lower() for t in titles]
        return [(t, at) for t, at in self.seen.items() if any(w in t.lower() for w in wanted)]


class EventUiWatcher:
    """A ui check timed from an event (power_off:2): from the moment the
    event file appears, poll the app's tree until the assertion holds or
    within_s passes. Runs alongside the agent."""

    def __init__(self, machine: Machine, body: dict, variables: dict):
        self.m = machine
        self.body = render(body, variables)
        kind, n = body["after_event"].split(":")
        self.event_path = machine.t.path(machine.events_dir, f"{kind}-{n}")
        self.within = float(body.get("within_s", 60))
        self.result: tuple[bool, str] | None = None
        self.event_at: float | None = None
        self._stop = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)
        self._mcp: Mcp | None = None

    def start(self):
        self._mcp = self.m.open_mcp("event-ui")
        self.thread.start()

    def _run(self):
        while not self._stop.is_set():
            raw = self.m.t.read(self.event_path)
            if raw:
                try:
                    self.event_at = float(raw.strip())
                except ValueError:
                    self.event_at = time.time()
                break
            self._stop.wait(2)
        if self.event_at is None:
            return
        last = "not read"
        while True:
            tree, where = app_tree(self._mcp)
            took = time.time() - self.event_at
            if tree is not None:
                keep_tree(self.m, self.body.get("name") or "event-ui", tree)
                ok, ev = ui_assert(tree, self.body)
                last = f"{ev} ({where})"
                if ok:
                    self.result = (took <= self.within, f"after {took:.0f} s: {last}")
                    return
            if took > self.within:
                self.result = (False, f"not within {self.within:.0f} s of the event: {last}")
                return
            time.sleep(3)

    def finish(self, grace_s: float) -> tuple[bool, str]:
        self.thread.join(timeout=grace_s)
        self._stop.set()
        if self._mcp:
            self._mcp.close()
        if self.event_at is None:
            return False, f"the event {self.body['after_event']} never happened"
        if self.result is None:
            return False, "still waiting when the run ended"
        return self.result


def run_checks(
    machine: Machine,
    checks: list[dict],
    variables: dict,
    dialogs: DialogWatcher,
    event_watchers: dict[int, EventUiWatcher],
    relay_base: str,
) -> list[CheckResult]:
    out: list[CheckResult] = []
    for i, raw in enumerate(checks):
        kind = raw["type"]
        name = raw.get("name") or f"{kind} #{i + 1}"
        only = raw.get("os")
        if only and machine.t.os not in _list(only):
            out.append(
                CheckResult(kind, name, True, f"not checked on {machine.t.os}", skipped=True)
            )
            continue
        try:
            if i in event_watchers:
                ok, ev = event_watchers[i].finish(grace_s=float(raw.get("within_s", 60)) + 30)
                out.append(CheckResult(kind, name, ok, ev))
                continue
            body = render(raw, variables)
            ok, ev = _one(machine, kind, body, dialogs, relay_base)
        except Exception as e:  # a check that cannot run is a failed check
            ok, ev = False, f"{type(e).__name__}: {e}"
        out.append(CheckResult(kind, name, ok, ev))
    return out


def _one(
    m: Machine, kind: str, body: dict, dialogs: DialogWatcher, relay_base: str
) -> tuple[bool, str]:
    within = float(body.get("within_s", DEFAULT_WITHIN_S))
    if kind == "relay":

        def f():
            status, doc = canary.relay_request(m.session, relay_base, "GET", body["get"])
            if status != 200:
                return False, f"GET {body['get']} -> {status}"
            ok, out = jq_ok(body["jq"], doc)
            return ok, f"jq {body['jq']!r} -> {out}"

        return _poll(f, within, 5)
    if kind == "cli":
        argv = body["run"].split() if isinstance(body["run"], str) else list(body["run"])
        n = int(body.get("host", 1))

        def f():
            r = m.cua(n, argv[1:], timeout=120)
            try:
                doc = first_json(r.out)
            except ValueError:
                return (
                    False,
                    f"`{' '.join(argv)}` exit {r.rc}, no JSON: {(r.err or r.out).strip()[-200:]}",
                )
            ok, out = jq_ok(body["jq"], doc)
            return ok, f"host {n} `{' '.join(argv)}` | jq {body['jq']!r} -> {out}"

        return _poll(f, within, 5)
    if kind == "ui":
        hold = float(body.get("hold_s", 0))

        def f():
            tree, where = app_tree(m.mcp)
            if tree is None:
                return False, where
            keep_tree(m, body.get("name") or "ui", tree)
            ok, ev = ui_assert(tree, body)
            return ok, f"{ev} ({where})"

        ok, ev = _poll(f, within, 3)
        if not ok or hold <= 0:
            return ok, ev
        end = time.monotonic() + hold
        while time.monotonic() < end:
            time.sleep(3)
            ok2, ev2 = f()
            if not ok2:
                return False, f"held, then broke within {hold:.0f} s: {ev2}"
        return True, f"{ev}; held {hold:.0f} s"
    if kind == "no_dialogs":
        hits = dialogs.matches(_list(body["titles"]))
        if hits:
            return False, "appeared: " + "; ".join(f"{t!r} at {at} s" for t, at in hits)
        return (
            True,
            f"none of {len(_list(body['titles']))} titles appeared ({len(dialogs.seen)} window titles seen)",
        )
    if kind == "file":
        n = int(body.get("host", 1))
        path = body["path"]
        text = m.t.read(path)
        want_exists = body.get("exists", True)
        if text is None:
            return (not want_exists), f"host {n}: {path} does not exist"
        if not want_exists:
            return False, f"host {n}: {path} exists"
        for s in _list(body.get("contains")):
            if s not in text:
                return False, f"{path} lacks {s!r}"
        for s in _list(body.get("absent")):
            if s in text:
                return False, f"{path} contains {s!r}"
        return True, f"{path} as expected"
    raise ValueError(kind)


def save(results: list[CheckResult], path: Path) -> None:
    path.write_text(json.dumps([r.__dict__ for r in results], indent=2), encoding="utf-8")

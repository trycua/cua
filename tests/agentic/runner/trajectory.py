"""Record and replay.

A passing live run is saved as a trajectory: the turn folders Cua Driver's
start_recording wrote (`turn-NNNNN/action.json`, the same files
replay_trajectory reads), plus `portable.json`, which says for every turn
which app and window its pid and window_id meant and which row each
element_token pointed at. Screenshots and video stay in the run's artifacts;
only the actions are committed.

replay_trajectory re-sends the recorded arguments verbatim, so a pid, a
window_id or an element token from the recording fails against a new app
process. Replay here reads the same turn files and maps those three to the
live app first: pid and window_id by app name and window title, a token by
re-reading the window and finding the row with the same text. A turn that
cannot be mapped, a tool error, or a failed check after the last turn means
the replay broke, and the runner falls back to a live agent run.
"""

from __future__ import annotations

import copy
import json
import re
import shutil
import time
from pathlib import Path

from .mcp import Mcp, rows, tree_text, window_app, window_title, windows

TOKEN = re.compile(r"^([A-Za-z0-9_-]+):(\d+)$")
ROOT = Path(__file__).resolve().parents[1] / "trajectories"


def trajectory_dir(area: str, name: str, os_name: str, root: Path = ROOT) -> Path:
    return root / area / name / os_name


def _load_jsonl(p: Path) -> list[dict]:
    if not p.exists():
        return []
    out = []
    for line in p.read_text(encoding="utf-8").splitlines():
        try:
            out.append(json.loads(line))
        except ValueError:
            pass
    return out


def _templatize(v, variables: dict[str, str]):
    """Run-specific values (host names, machine ids) back to {{variables}}."""
    pairs = sorted(
        ((val, key) for key, val in variables.items() if val and len(val) >= 4),
        key=lambda p: -len(p[0]),
    )
    if isinstance(v, str):
        for val, key in pairs:
            v = v.replace(val, "{{" + key + "}}")
        return v
    if isinstance(v, list):
        return [_templatize(x, variables) for x in v]
    if isinstance(v, dict):
        return {k: _templatize(x, variables) for k, x in v.items()}
    return v


def build_portable(
    run_dir: Path, recording: Path, prompt: str, variables: dict[str, str] | None = None
) -> dict:
    """Map each recorded turn's pid, window_id and tokens to what they meant.
    Run-specific values are stored as {{variables}}, filled again at replay."""
    variables = variables or {}
    calls = _load_jsonl(run_dir / "calls.jsonl")
    snaps: dict[str, dict] = {}
    for snap in _load_jsonl(run_dir / "snapshots.jsonl"):
        # A diff read (`since`) lists only changed rows: merge, so a row
        # still on screen keeps the text an earlier read gave it.
        prev = snaps.get(snap["snapshot_id"])
        if prev and len(prev.get("tree") or "") > len(snap.get("tree") or ""):
            continue
        snaps[snap["snapshot_id"]] = snap
    pid_app: dict[int, str] = {}
    win_title: dict[int, str] = {}
    for c in calls:
        st = c.get("structured") or {}
        for w in st.get("windows") or []:
            if isinstance(w, dict) and "pid" in w:
                pid_app[w["pid"]] = window_app(w)
                if "window_id" in w:
                    win_title[w["window_id"]] = window_title(w)
        if c.get("tool") == "launch_app" and st.get("pid"):
            pid_app[st["pid"]] = str(
                c.get("args", {}).get("name")
                or c.get("args", {}).get("app")
                or st.get("app_name")
                or ""
            )

    def row_text(token: str) -> dict | None:
        m = TOKEN.match(token)
        if not m or m.group(1) not in snaps:
            return None
        snap = snaps[m.group(1)]
        for idx, text in rows(snap.get("tree") or ""):
            if idx == int(m.group(2)):
                return {
                    "row": text.strip(),
                    "pid": snap.get("pid"),
                    "window_id": snap.get("window_id"),
                }
        return None

    turns = []
    replayable, why = True, ""
    screen_text = "\n".join(s.get("tree") or "" for s in snaps.values())
    for d in sorted(recording.glob("turn-*")):
        action = json.loads((d / "action.json").read_text(encoding="utf-8"))
        args = action.get("arguments") or action.get("args") or {}
        tokens = {}
        for tok in _find_tokens(args):
            info = row_text(tok)
            if info is None:
                replayable, why = (
                    False,
                    f"{d.name}: element token {tok} not in any snapshot the agent read",
                )
            tokens[tok] = info
        pids = {p: pid_app.get(p, "") for p in _find_key(args, "pid")}
        wins = {w: win_title.get(w, "") for w in _find_key(args, "window_id")}
        for info in tokens.values():
            # A token alone names its window through its snapshot.
            if info and isinstance(info.get("pid"), int):
                pids.setdefault(info["pid"], pid_app.get(info["pid"], ""))
            if info and isinstance(info.get("window_id"), int):
                wins.setdefault(info["window_id"], win_title.get(info["window_id"], ""))
        typed = [v for v in _find_key(args, "text") if isinstance(v, str)]
        for t in typed:
            # Text copied off the screen (a pairing code) differs every run.
            if "{{" in _templatize(t, variables):
                continue  # a host name or id: filled in again at replay
            if len(t.strip()) >= 6 and t.strip() not in prompt and t.strip() in screen_text:
                replayable, why = (
                    False,
                    f"{d.name}: types text read off the screen ({t.strip()[:20]!r}), which changes every run",
                )
        turns.append(
            {
                "turn": d.name,
                "tool": action.get("tool"),
                "at": action.get("timestamp"),
                "args": _templatize(args, variables),
                "recorded_error": bool(action.get("result_error")),
                "pids": {str(k): v for k, v in pids.items()},
                "windows": {str(k): _templatize(v, variables) for k, v in wins.items()},
                "tokens": _templatize(tokens, variables),
            }
        )
    return {"turns": turns, "replayable": replayable, "not_replayable_because": why}


def _find_tokens(v) -> list[str]:
    out = []
    if isinstance(v, dict):
        for k, x in v.items():
            if k == "element_token" and isinstance(x, str):
                out.append(x)
            else:
                out += _find_tokens(x)
    elif isinstance(v, list):
        for x in v:
            out += _find_tokens(x)
    return out


def _find_key(v, key: str) -> list:
    out = []
    if isinstance(v, dict):
        for k, x in v.items():
            if k == key and isinstance(x, (int, str)):
                out.append(x)
            else:
                out += _find_key(x, key)
    elif isinstance(v, list):
        for x in v:
            out += _find_key(x, key)
    return out


def save(dest: Path, recording: Path, portable: dict, manifest: dict) -> None:
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    for d in sorted(recording.glob("turn-*")):
        (dest / d.name).mkdir()
        shutil.copy2(d / "action.json", dest / d.name / "action.json")
    (dest / "portable.json").write_text(json.dumps(portable, indent=2) + "\n", encoding="utf-8")
    (dest / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


class ReplayBroke(RuntimeError):
    pass


def replay(mcp: Mcp, src: Path, log, variables: dict[str, str] | None = None) -> int:
    """Replay a saved trajectory on the live machine. Returns turns replayed."""
    from .spec import render

    portable = json.loads((src / "portable.json").read_text(encoding="utf-8"))
    portable = render(portable, variables or {})
    if not portable.get("replayable", True):
        raise ReplayBroke("not replayable: " + portable.get("not_replayable_because", ""))
    last_at = None
    n = 0
    for t in portable["turns"]:
        if t.get("recorded_error"):
            continue  # the agent's own failed attempt: nothing to repeat
        action = json.loads((src / t["turn"] / "action.json").read_text(encoding="utf-8"))
        tool = action.get("tool")
        args = copy.deepcopy(t.get("args") if "args" in t else (action.get("arguments") or {}))
        # Keep the recording's pacing (capped), so the app has the time it had.
        at = _ts(t.get("at"))
        if last_at is not None and at is not None:
            time.sleep(min(max(at - last_at, 0.5), 8.0))
        last_at = at
        pid_map, win_map = _map_windows(mcp, t)
        args = _remap(args, pid_map, win_map)
        for tok, info in (t.get("tokens") or {}).items():
            if not info:
                raise ReplayBroke(f"{t['turn']}: token {tok} has no recorded row")
            new = _find_row(mcp, info, pid_map, win_map)
            if not new:
                raise ReplayBroke(f"{t['turn']}: no row {info['row'][:80]!r} on screen now")
            args = _replace_token(args, tok, new)
        if (
            "x" in args
            and "y" in args
            and args.get("pid")
            and args.get("window_id")
            and args.get("scope") != "desktop"
        ):
            # Pixel coordinates are relative to the window's last screenshot:
            # take one, as the agent did before it acted.
            mcp.call(
                "get_window_state",
                {
                    "pid": args["pid"],
                    "window_id": args["window_id"],
                    "include_accessibility_tree": False,
                },
                timeout=60,
            )
        r = mcp.call(tool, args, timeout=120)
        log(f"replay {t['turn']} {tool}: {'error' if r.is_error else 'ok'} {r.first_line()[:120]}")
        if r.is_error:
            raise ReplayBroke(f"{t['turn']} {tool}: {r.first_line()}")
        n += 1
    return n


def _ts(s) -> float | None:
    if not s:
        return None
    try:
        from datetime import datetime

        return datetime.fromisoformat(str(s).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def _map_windows(mcp: Mcp, t: dict) -> tuple[dict, dict]:
    live = windows(mcp)
    pid_map, win_map = {}, {}
    for old_pid, app in (t.get("pids") or {}).items():
        cands = [w for w in live if app and window_app(w).lower() == app.lower()]
        if not cands:
            raise ReplayBroke(f"{t['turn']}: no window of {app or 'pid ' + old_pid} now")
        pid_map[int(old_pid)] = cands[0]["pid"]
    for old_win, title in (t.get("windows") or {}).items():
        cands = [w for w in live if title and window_title(w) == title]
        if not cands and pid_map:
            # No title recorded: the turn's own app's largest window.
            mine = set(pid_map.values()) if len(pid_map) == 1 else set()
            cands = sorted((w for w in live if w["pid"] in mine), key=_area, reverse=True)
        if not cands:
            raise ReplayBroke(f"{t['turn']}: no window titled {title!r} now")
        win_map[int(old_win)] = cands[0]["window_id"]
        pid_map.setdefault(-1, cands[0]["pid"])
    return pid_map, win_map


def _area(w: dict) -> float:
    b = w.get("bounds") or {}
    return float(b.get("width", 0)) * float(b.get("height", 0))


def _remap(v, pid_map, win_map):
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if k == "pid" and isinstance(x, int) and x in pid_map:
                out[k] = pid_map[x]
            elif k == "window_id" and isinstance(x, int) and x in win_map:
                out[k] = win_map[x]
            else:
                out[k] = _remap(x, pid_map, win_map)
        return out
    if isinstance(v, list):
        return [_remap(x, pid_map, win_map) for x in v]
    return v


def _replace_token(v, old: str, new: str):
    if isinstance(v, dict):
        return {
            k: (new if k == "element_token" and x == old else _replace_token(x, old, new))
            for k, x in v.items()
        }
    if isinstance(v, list):
        return [_replace_token(x, old, new) for x in v]
    return v


ROW_KEY = re.compile(r'^\s*([^"\[]*?)\s*"([^"]*)"')


def row_key(text: str) -> tuple[str, str]:
    """A row's role and label: what identifies it across reads and formats."""
    m = ROW_KEY.match(text.strip())
    role, label = (m.group(1).strip().lower(), m.group(2)) if m else (text.strip().lower(), "")
    # Counts in a label ("Machines 10 waiting to be added") change between
    # runs; the words do not.
    return role, " ".join(re.sub(r"\d+", " ", label).split())


def _find_row(mcp: Mcp, info: dict, pid_map: dict, win_map: dict) -> str | None:
    pid = pid_map.get(info.get("pid"), info.get("pid"))
    win = win_map.get(info.get("window_id"), info.get("window_id"))
    label = re.findall(r'"([^"]+)"', info["row"])
    words = re.findall(r"[^\W\d]{3,}", label[0]) if label else []
    query = words[0] if words else (label[0] if label else info["row"][:40])
    deadline = time.monotonic() + 10
    while True:
        r = mcp.call(
            "get_window_state",
            {
                "pid": pid,
                "window_id": win,
                "query": query,
                "include_screenshot": False,
                "max_elements": 2000,
            },
            timeout=60,
        )
        if not r.is_error:
            snap = r.structured.get("snapshot_id")
            want = row_key(info["row"])
            for idx, text in rows(tree_text(r)):
                if row_key(text) == want:
                    return f"{snap}:{idx}"
        if time.monotonic() > deadline:
            return None
        time.sleep(1)

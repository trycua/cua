import json
import random
import re
from pathlib import Path
from typing import Any, Dict, List

import cua_bench as cb
from datasets import load_from_disk

# Globals for the task process
dataset = load_from_disk(Path(__file__).parent / "traces")
_rows: List[Dict[str, Any]] = []


# Called once per batch
@cb.tasks_config(split="train")
def load():
    target_os_types = ["win11", "win10", "macos", "ios", "android"]
    trajectory_ids = set(dataset["trajectory_id"])
    return [
        cb.Task(
            description="Replay a recorded trace and visualize snapshots.",
            metadata={"os_type": os_type, "trajectory_id": tid},
        )
        for os_type in target_os_types
        for tid in trajectory_ids
    ]


# Called at start of task
@cb.setup_task(split="train")
def start(task_cfg, env: cb.DesktopSession | cb.MobileSession):
    global _rows
    # Filter rows by trajectory_id and sort by timestamp string (ISO8601)
    tid = task_cfg.metadata.get("trajectory_id")
    all_rows = [dict(r) for r in dataset]
    if tid is not None:
        all_rows = [r for r in all_rows if str(r.get("trajectory_id")) == str(tid)]
    _rows = sorted(all_rows, key=lambda r: str(r.get("timestamp") or ""))
    print(f"Loaded {len(_rows)} rows for trajectory_id={tid}")

    # Set sandbox dimensions based on OS type
    os_type = task_cfg.metadata["os_type"]
    width, height = (1024, 768) if os_type not in ("ios", "android") else (384, 640)
    seed = random.randint(0, 2**32 - 1)

    # Create a minimal sandbox for replay
    env.create_sandbox(
        provider="webtop",
        setup_config={
            "os_type": os_type,
            "width": width,
            "height": height,
            "background": f"url('https://picsum.photos/seed/{seed}/{width}/{height}')",
            "randomize_apps": True,
        },
    )

    # Find first valid snapshot
    first_snap = next(
        (
            snap
            for row in _rows
            if (data := json.loads(row.get("data_json")))
            and "snapshot" in data
            and isinstance(snap := data.get("snapshot"), dict)
            and isinstance(snap.get("windows"), list)
        ),
        None,
    )

    # Render if found
    if first_snap:
        _render_snapshot(env, first_snap)


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
def solve(task_cfg, env: cb.DesktopSession | cb.MobileSession):
    global _rows
    # Walk rows, stepping actions and rendering subsequent snapshots
    for row in _rows:
        name = row.get("event_name")

        if name in ("step:before", "step:after"):
            # render snapshot from row if present
            data = json.loads(row["data_json"])
            _render_snapshot(env, data["snapshot"])

            # extract action
            act_repr = data["action"]
            action = cb.repr_to_action(act_repr)

            # step action
            print(f"Event: {name} | Action: {act_repr}")
            env.step(action, dry_run=name.split(":")[1])


# Called at end of task
@cb.evaluate_task(split="train")
def evaluate(task_cfg, env: cb.DesktopSession | cb.MobileSession) -> List[float]:
    global _rows
    # get last evaluate event
    evaluate_event = next(
        (row for row in reversed(_rows) if row.get("event_name") == "evaluate"), None
    )
    if evaluate_event is None:
        return []

    # extract result
    result = json.loads(evaluate_event["data_json"]).get("result", [])
    return result


# --- helper functions ---
def _render_snapshot(env, snap: Dict[str, Any]) -> None:
    # close all windows
    env.close_all_windows()
    # launch windows
    windows = (snap or {}).get("windows") or []
    for w in windows:
        html = w.get("html") or ""
        html = strip_scripts(html)
        title = w.get("title") or "Window"
        x = int(w.get("x") or 100)
        y = int(w.get("y") or 100)
        width = int(w.get("width") or 400)
        height = int(w.get("height") or 300)
        env.launch_window(
            html=html,
            title=title,
            x=x,
            y=y,
            width=width,
            height=height,
            use_inner_size=False,
            title_bar_style="default",
        )


_script_re = re.compile(r"<script\b[^>]*>.*?</script>", re.IGNORECASE | re.DOTALL)


def strip_scripts(s: str) -> str:
    return _script_re.sub("", s)


if __name__ == "__main__":
    cb.interact(__file__)

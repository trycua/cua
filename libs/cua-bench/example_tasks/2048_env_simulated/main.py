from pathlib import Path

import cua_bench as cb
from cua_bench.types import KeyAction


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["win11", "win10", "macos"]
    return [
        cb.Task(
            description="Play 2048 and try to maximize the tile.",
            metadata={
                "os_type": os_type,
            },
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": os_type,
                    "width": 768,
                    "height": 768,
                    "background": "#202020",
                    "randomize_apps": True,
                },
            },
        )
        for os_type in os_types
    ]


# All code below will be running in a separate process per task

pid = None


# Called at start of task
@cb.setup_task(split="train")
def start(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    global pid
    pid = session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text(),
        title="2048 Env",
        width=512,
        height=512,
    )


# Called at end of task
@cb.evaluate_task(split="train")
def evaluate(task_cfg, session: cb.DesktopSession | cb.MobileSession) -> list[float]:
    # Simple metric: normalize by max tile seen (0..2048 -> 0..1). Not strict.
    if pid is None:
        return [0.0]
    try:
        m = session.execute_javascript(pid, "window.__max_tile || 0") or 0
        score = min(max(int(m), 0), 2048) / 2048.0
        return [float(score)]
    except Exception:
        return [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
def solve(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    global pid
    if pid is None:
        return
    # Greedy auto-player using window.__next_move()
    for _ in range(200):
        lost = session.execute_javascript(pid, "window.__lost === true")
        if lost:
            break
        direction = session.execute_javascript(
            pid, "(window.__next_move && window.__next_move()) || null"
        )
        if not direction:
            break
        key = {
            "left": "left",
            "right": "right",
            "up": "up",
            "down": "down",
        }.get(direction)
        if not key:
            break
        if hasattr(session, "env") and session.env:
            session.env.step(KeyAction(key=key))

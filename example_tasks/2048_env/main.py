import cua_bench as cb
from pathlib import Path
from cua_bench.types import KeyAction

# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]
    return [
        cb.Task(
            description="Play 2048 and try to maximize the tile.",
            metadata={
                "os_type": os_type,
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": os_type,
                    "width": 768,
                    "height": 768,
                    "background": '#202020',
                }
            }
        )
        for os_type in os_types
    ]

# All code below will be running in a separate process per task
pid = -1

# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    global pid
    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text(),
        title="2048 Env",
        width=512,
        height=512,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession) -> list[float]:
    # Simple metric: normalize by max tile seen (0..2048 -> 0..1). Not strict.
    try:
        m = await session.execute_javascript(pid, "window.__max_tile || 0") or 0
        score = min(max(int(m), 0), 2048) / 2048.0
        return [float(score)]
    except Exception:
        return [0.0]

# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession | cb.MobileSession):
    global pid
    if pid is None:
        return
    # Greedy auto-player using window.__next_move()
    for _ in range(200):
        lost = await session.execute_javascript(pid, "window.__lost === true")
        if lost:
            break
        direction = await session.execute_javascript(pid, "(window.__next_move && window.__next_move()) || null")
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
        await session.execute_action(KeyAction(key=key))
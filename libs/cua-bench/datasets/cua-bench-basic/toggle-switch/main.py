import cua_bench as cb
from pathlib import Path

# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"] # ["macos", "win11", "win10"]

    # Different toggle switch scenarios
    toggle_scenarios = [
        {"setting": "notifications", "label": "Notifications", "target_state": True, "description": "Turn on Notifications"},
        {"setting": "dark_mode", "label": "Dark Mode", "target_state": True, "description": "Enable Dark Mode"},
        {"setting": "auto_save", "label": "Auto Save", "target_state": False, "description": "Turn off Auto Save"},
        {"setting": "wifi", "label": "WiFi", "target_state": True, "description": "Enable WiFi"},
        {"setting": "bluetooth", "label": "Bluetooth", "target_state": False, "description": "Turn off Bluetooth"},
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "setting": scenario["setting"],
                "label": scenario["label"],
                "target_state": scenario["target_state"],
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": os_type,
                    "width": 1024,
                    "height": 768,
                    "background": '#c0c0c0'
                }
            }
        )
        for os_type in os_types
        for scenario in toggle_scenarios
    ]

# All code below will be running in a separate process per task

pid = None

# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Setup steps:
    # 1. Create a webview window
    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text('utf-8'),
        title="Toggle Switch Task",
        width=450,
        height=500,
    )

# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Get the toggle states
    toggle_states = await session.execute_javascript(pid, "window.__toggleStates")

    if toggle_states is None:
        return [0.0]

    # 2. Verify the correct toggle is in the correct state
    setting = task_cfg.metadata["setting"]
    target_state = task_cfg.metadata["target_state"]

    actual_state = toggle_states.get(setting)
    return [1.0] if actual_state == target_state else [0.0]

# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Get the current state of the toggle
    setting = task_cfg.metadata["setting"]
    target_state = task_cfg.metadata["target_state"]

    current_state = await session.execute_javascript(pid, f"window.__toggleStates['{setting}']")

    # 2. Click the toggle if it's not already in the target state
    if current_state != target_state:
        await session.click_element(pid, f"#toggle-{setting}")

if __name__ == "__main__":
    cb.interact(__file__)

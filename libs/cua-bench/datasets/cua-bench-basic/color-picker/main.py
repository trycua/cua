import cua_bench as cb
from pathlib import Path

# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"] # ["macos", "win11", "win10"]

    # Different color picking scenarios
    color_scenarios = [
        {"color": "#FF0000", "name": "red", "description": "Select the red color"},
        {"color": "#00FF00", "name": "green", "description": "Select the green color"},
        {"color": "#0000FF", "name": "blue", "description": "Select the blue color"},
        {"color": "#FFFF00", "name": "yellow", "description": "Select the yellow color"},
        {"color": "#FF00FF", "name": "magenta", "description": "Select the magenta color"},
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "color": scenario["color"],
                "name": scenario["name"],
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
        for scenario in color_scenarios
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
        title="Color Picker Task",
        width=500,
        height=450,
    )

# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Get the selected color
    selected_color = await session.execute_javascript(pid, "window.__selectedColor")

    if selected_color is None:
        return [0.0]

    # 2. Verify the correct color was selected (normalize to uppercase)
    expected_color = task_cfg.metadata["color"].upper()
    selected_color = selected_color.upper()

    return [1.0] if selected_color == expected_color else [0.0]

# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Click the target color button
    color_name = task_cfg.metadata["name"]
    await session.click_element(pid, f"#color-{color_name}")

if __name__ == "__main__":
    cb.interact(__file__)

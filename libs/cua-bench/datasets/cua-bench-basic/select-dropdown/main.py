import cua_bench as cb
from pathlib import Path

# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"] # ["macos", "win11", "win10"]

    # Different dropdown selection scenarios
    dropdown_scenarios = [
        {"value": "apple", "label": "Apple", "description": 'Select "Apple" from the dropdown'},
        {"value": "banana", "label": "Banana", "description": 'Select "Banana" from the dropdown'},
        {"value": "orange", "label": "Orange", "description": 'Select "Orange" from the dropdown'},
        {"value": "grape", "label": "Grape", "description": 'Select "Grape" from the dropdown'},
        {"value": "mango", "label": "Mango", "description": 'Select "Mango" from the dropdown'},
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "value": scenario["value"],
                "label": scenario["label"],
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
        for scenario in dropdown_scenarios
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
        title="Select Dropdown Task",
        width=400,
        height=350,
    )

# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Get the selected value
    selected_value = await session.execute_javascript(pid, "window.__selectedValue")

    if selected_value is None:
        return [0.0]

    # 2. Verify the correct option was selected
    expected_value = task_cfg.metadata["value"]
    return [1.0] if selected_value == expected_value else [0.0]

# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Click the dropdown to open it
    await session.click_element(pid, "#fruit-select")

    # 2. Click the target option
    target_value = task_cfg.metadata["value"]
    await session.click_element(pid, f'option[value="{target_value}"]')

if __name__ == "__main__":
    cb.interact(__file__)

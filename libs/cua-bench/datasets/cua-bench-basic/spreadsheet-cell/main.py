from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Different spreadsheet cell entry scenarios
    cell_scenarios = [
        {"cell": "A1", "value": "Product", "description": 'Enter "Product" into cell A1'},
        {"cell": "B2", "value": "150", "description": 'Enter "150" into cell B2'},
        {"cell": "C3", "value": "=SUM(A1:A10)", "description": 'Enter "=SUM(A1:A10)" into cell C3'},
        {
            "cell": "D4",
            "value": "Total Revenue",
            "description": 'Enter "Total Revenue" into cell D4',
        },
        {"cell": "A5", "value": "42.50", "description": 'Enter "42.50" into cell A5'},
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "cell": scenario["cell"],
                "value": scenario["value"],
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": os_type,
                    "width": 1024,
                    "height": 768,
                    "background": "#c0c0c0",
                },
            },
        )
        for os_type in os_types
        for scenario in cell_scenarios
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
        html=(Path(__file__).parent / "gui/index.html").read_text("utf-8"),
        title="Spreadsheet Task",
        width=700,
        height=500,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Get the cell data from the spreadsheet
    cell_data = await session.execute_javascript(pid, "window.__cellData")

    if cell_data is None:
        return [0.0]

    # 2. Check if the correct cell has the correct value
    target_cell = task_cfg.metadata["cell"]
    expected_value = task_cfg.metadata["value"]

    actual_value = cell_data.get(target_cell)

    # Return 1.0 if the value matches exactly, 0.0 otherwise
    return [1.0] if actual_value == expected_value else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Click the target cell
    target_cell = task_cfg.metadata["cell"]
    await session.click_element(pid, f"#cell-{target_cell}")

    # 2. Type the value
    await session.execute_action(cb.TypeAction(text=task_cfg.metadata["value"]))

    # 3. Press Enter to confirm the entry
    await session.execute_action(cb.KeyAction(key="Return"))


if __name__ == "__main__":
    cb.interact(__file__)

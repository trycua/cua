from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Different date picking scenarios
    date_scenarios = [
        {"date": "2024-01-15", "description": "Select January 15, 2024"},
        {"date": "2024-06-20", "description": "Select June 20, 2024"},
        {"date": "2024-12-25", "description": "Select December 25, 2024"},
        {"date": "2025-03-10", "description": "Select March 10, 2025"},
        {"date": "2025-09-05", "description": "Select September 5, 2025"},
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "date": scenario["date"],
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
        for scenario in date_scenarios
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
        title="Date Picker Task",
        width=450,
        height=400,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Get the selected date
    selected_date = await session.execute_javascript(pid, "window.__selectedDate")

    if selected_date is None:
        return [0.0]

    # 2. Verify the correct date was selected
    expected_date = task_cfg.metadata["date"]
    return [1.0] if selected_date == expected_date else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Click the date input to open the picker
    await session.click_element(pid, "#date-input")

    # 2. Type the date (most date inputs accept keyboard input)
    await session.execute_action(cb.TypeAction(text=task_cfg.metadata["date"]))

    # 3. Press Enter to confirm
    await session.execute_action(cb.KeyAction(key="Return"))


if __name__ == "__main__":
    cb.interact(__file__)

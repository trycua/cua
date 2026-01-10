from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Different icon clicking scenarios
    icon_scenarios = [
        {"icon": "mdi:home", "name": "Home", "description": "Click the Home icon"},
        {"icon": "mdi:settings", "name": "Settings", "description": "Click the Settings icon"},
        {"icon": "mdi:account", "name": "Profile", "description": "Click the Profile icon"},
        {
            "icon": "mdi:bell",
            "name": "Notifications",
            "description": "Click the Notifications icon",
        },
        {"icon": "mdi:email", "name": "Messages", "description": "Click the Messages icon"},
        {"icon": "mdi:star", "name": "Favorites", "description": "Click the Favorites icon"},
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "icon": scenario["icon"],
                "name": scenario["name"],
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
        for scenario in icon_scenarios
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
        title="Icon Click Task",
        width=500,
        height=400,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Check which icon was clicked
    clicked_icon = await session.execute_javascript(pid, "window.__clickedIcon")

    if clicked_icon is None:
        return [0.0]

    # 2. Verify the correct icon was clicked
    expected_name = task_cfg.metadata["name"]
    return [1.0] if clicked_icon == expected_name else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Click the target icon
    icon_name = task_cfg.metadata["name"]
    await session.click_element(pid, f"#icon-{icon_name}")


if __name__ == "__main__":
    cb.interact(__file__)

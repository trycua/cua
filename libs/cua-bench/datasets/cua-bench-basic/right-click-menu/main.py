from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Different right-click menu action scenarios
    menu_scenarios = [
        {"action": "copy", "label": "Copy", "description": "Right-click the text and select Copy"},
        {
            "action": "paste",
            "label": "Paste",
            "description": "Right-click the text and select Paste",
        },
        {"action": "cut", "label": "Cut", "description": "Right-click the text and select Cut"},
        {
            "action": "delete",
            "label": "Delete",
            "description": "Right-click the text and select Delete",
        },
        {
            "action": "select_all",
            "label": "Select All",
            "description": "Right-click the text and select Select All",
        },
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "action": scenario["action"],
                "label": scenario["label"],
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
        for scenario in menu_scenarios
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
        title="Right-Click Menu Task",
        width=500,
        height=400,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Get the selected menu action
    selected_action = await session.execute_javascript(pid, "window.__selectedAction")

    if selected_action is None:
        return [0.0]

    # 2. Verify the correct action was selected
    expected_action = task_cfg.metadata["action"]
    return [1.0] if selected_action == expected_action else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Right-click on the target area to open context menu
    # Get the coordinates of the text area
    text_area_info = await session.execute_javascript(
        pid,
        """
        (function() {
            const textArea = document.getElementById('context-area');
            const rect = textArea.getBoundingClientRect();
            return {
                x: rect.left + window.screenX + rect.width / 2,
                y: rect.top + window.screenY + rect.height / 2
            };
        })()
    """,
    )

    # Right-click to open menu
    await session.execute_action(cb.RightClickAction(x=text_area_info["x"], y=text_area_info["y"]))

    # 2. Click the target menu item
    action = task_cfg.metadata["action"]
    await session.click_element(pid, f"#menu-{action}")


if __name__ == "__main__":
    cb.interact(__file__)

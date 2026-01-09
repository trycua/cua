from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Different drag and drop scenarios
    drag_scenarios = [
        {
            "item": "apple",
            "target": "fruit",
            "item_label": "Apple",
            "target_label": "Fruits",
            "description": "Drag the Apple to the Fruits box",
        },
        {
            "item": "carrot",
            "target": "vegetable",
            "item_label": "Carrot",
            "target_label": "Vegetables",
            "description": "Drag the Carrot to the Vegetables box",
        },
        {
            "item": "banana",
            "target": "fruit",
            "item_label": "Banana",
            "target_label": "Fruits",
            "description": "Drag the Banana to the Fruits box",
        },
        {
            "item": "broccoli",
            "target": "vegetable",
            "item_label": "Broccoli",
            "target_label": "Vegetables",
            "description": "Drag the Broccoli to the Vegetables box",
        },
        {
            "item": "orange",
            "target": "fruit",
            "item_label": "Orange",
            "target_label": "Fruits",
            "description": "Drag the Orange to the Fruits box",
        },
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "item": scenario["item"],
                "target": scenario["target"],
                "item_label": scenario["item_label"],
                "target_label": scenario["target_label"],
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
        for scenario in drag_scenarios
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
        title="Drag and Drop Task",
        width=600,
        height=500,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Get the drop results
    drop_results = await session.execute_javascript(pid, "window.__dropResults")

    if drop_results is None:
        return [0.0]

    # 2. Check if the correct item was dropped in the correct target
    item = task_cfg.metadata["item"]
    target = task_cfg.metadata["target"]

    # Check if the item is in the correct target box
    item_location = drop_results.get(item)
    return [1.0] if item_location == target else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Get the coordinates of the item and target
    item = task_cfg.metadata["item"]
    target = task_cfg.metadata["target"]

    coords = await session.execute_javascript(
        pid,
        f"""
        (function() {{
            const itemEl = document.getElementById('item-{item}');
            const targetEl = document.getElementById('drop-{target}');

            const itemRect = itemEl.getBoundingClientRect();
            const targetRect = targetEl.getBoundingClientRect();

            return {{
                item_x: itemRect.left + window.screenX + itemRect.width / 2,
                item_y: itemRect.top + window.screenY + itemRect.height / 2,
                target_x: targetRect.left + window.screenX + targetRect.width / 2,
                target_y: targetRect.top + window.screenY + targetRect.height / 2
            }};
        }})()
    """,
    )

    # 2. Drag the item to the target
    await session.execute_action(
        cb.DragAction(
            from_x=coords["item_x"],
            from_y=coords["item_y"],
            to_x=coords["target_x"],
            to_y=coords["target_y"],
            duration=0.5,
        )
    )


if __name__ == "__main__":
    cb.interact(__file__)

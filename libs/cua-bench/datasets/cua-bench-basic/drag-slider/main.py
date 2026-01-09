from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Different slider scenarios with various target values
    slider_scenarios = [
        {"target_value": 25, "label": "Volume"},
        {"target_value": 50, "label": "Brightness"},
        {"target_value": 75, "label": "Temperature"},
        {"target_value": 10, "label": "Speed"},
        {"target_value": 90, "label": "Quality"},
    ]

    return [
        cb.Task(
            description=f'Set the {scenario["label"]} slider to {scenario["target_value"]}.',
            metadata={
                "target_value": scenario["target_value"],
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
        for scenario in slider_scenarios
    ]


# All code below will be running in a separate process per task

pid = None


# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Setup steps:
    # 1. Create a webview window with dynamic label
    html_template = (Path(__file__).parent / "gui/index.html").read_text("utf-8")
    html = html_template.replace("{{LABEL}}", task_cfg.metadata["label"])

    pid = await session.launch_window(
        html=html,
        title="Slider Task",
        width=500,
        height=300,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Check if the slider value matches the target
    slider_value = await session.execute_javascript(pid, "window.__sliderValue")

    if slider_value is None:
        return [0.0]

    # 2. Return a reward of 1.0 if the value matches, 0.0 otherwise
    # Allow for small tolerance (±2) due to slider precision
    expected_value = task_cfg.metadata["target_value"]
    tolerance = 2
    matches = abs(slider_value - expected_value) <= tolerance

    return [1.0] if matches else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Get the slider element coordinates
    slider_info = await session.execute_javascript(
        pid,
        """
        (function() {
            const slider = document.getElementById('slider');
            const rect = slider.getBoundingClientRect();
            return {
                x: rect.left + window.screenX,
                y: rect.top + window.screenY,
                width: rect.width,
                height: rect.height
            };
        })()
    """,
    )

    # 2. Calculate the target position based on the target value (0-100)
    target_value = task_cfg.metadata["target_value"]
    # Slider goes from 0 to 100, calculate position along the slider
    target_x = slider_info["x"] + (slider_info["width"] * target_value / 100)
    target_y = slider_info["y"] + slider_info["height"] / 2

    # 3. Drag the slider to the target position
    # Start from the current position (middle of slider)
    start_x = slider_info["x"] + slider_info["width"] / 2
    start_y = slider_info["y"] + slider_info["height"] / 2

    await session.execute_action(
        cb.DragAction(from_x=start_x, from_y=start_y, to_x=target_x, to_y=target_y, duration=0.5)
    )


if __name__ == "__main__":
    cb.interact(__file__)

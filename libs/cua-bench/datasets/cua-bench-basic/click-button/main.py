from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]
    button_texts = ["Submit", "Click Me", "Click Here", "OK", "Cancel", "Close", "Start"]
    return [
        cb.Task(
            description=f'Click the "{button_text}" button on the page.',
            metadata={
                "button_text": button_text,
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
        for button_text in button_texts
    ]


# All code below will be running in a separate process per task

pid = None


# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Setup steps:
    # 1. Create a webview window with dynamic button text
    html_template = (Path(__file__).parent / "gui/index.html").read_text("utf-8")
    html = html_template.replace("{{BUTTON_TEXT}}", task_cfg.metadata["button_text"])

    pid = await session.launch_window(
        html=html,
        title="Simple Task",
        width=256,
        height=256,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Check if the submit button was clicked by checking a global flag
    submitted = await session.execute_javascript(pid, "window.__submitted")
    # 2. Return a reward of 1.0 if submitted, 0.0 otherwise
    return [1.0] if submitted is True else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Click the button
    await session.click_element(pid, ".btn")


if __name__ == "__main__":
    cb.interact(__file__)

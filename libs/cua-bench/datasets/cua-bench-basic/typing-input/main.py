from pathlib import Path

import cua_bench as cb


# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"]  # ["macos", "win11", "win10"]

    # Different typing scenarios with various text inputs
    typing_scenarios = [
        {"text": "Hello World", "field_label": "Username"},
        {"text": "test@example.com", "field_label": "Email Address"},
        {"text": "My password is secure123!", "field_label": "Message"},
        {"text": "12345", "field_label": "Code"},
        {"text": "The quick brown fox jumps over the lazy dog", "field_label": "Text"},
    ]

    return [
        cb.Task(
            description=f'Type "{scenario["text"]}" into the {scenario["field_label"]} input field.',
            metadata={
                "text": scenario["text"],
                "field_label": scenario["field_label"],
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
        for scenario in typing_scenarios
    ]


# All code below will be running in a separate process per task

pid = None


# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Setup steps:
    # 1. Create a webview window with dynamic field label
    html_template = (Path(__file__).parent / "gui/index.html").read_text("utf-8")
    html = html_template.replace("{{FIELD_LABEL}}", task_cfg.metadata["field_label"])

    pid = await session.launch_window(
        html=html,
        title="Typing Input Task",
        width=400,
        height=300,
    )


# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Check if the input field contains the correct text
    input_value = await session.execute_javascript(pid, "window.__inputValue")

    # 2. Return a reward of 1.0 if the text matches exactly, 0.0 otherwise
    expected_text = task_cfg.metadata["text"]
    return [1.0] if input_value == expected_text else [0.0]


# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Solution steps:
    # 1. Click the input field
    await session.click_element(pid, "#text-input")

    # 2. Type the text
    await session.execute_action(cb.TypeAction(text=task_cfg.metadata["text"]))


if __name__ == "__main__":
    cb.interact(__file__)

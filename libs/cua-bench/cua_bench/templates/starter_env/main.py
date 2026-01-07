import cua_bench as cb
from pathlib import Path

# Global variable to track the window
pid = None


@cb.tasks_config(split="train")
def load():
    """Define task variants."""
    return [
        cb.Task(
            description='Click the "Submit" button on the page.',
            metadata={"button_text": "Submit"},
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": "macos",
                    "width": 512,
                    "height": 512,
                    "background": "#c0c0c0"
                }
            }
        )
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Initialize the task environment."""
    global pid

    html_content = (Path(__file__).parent / "gui/index.html").read_text()

    pid = await session.launch_window(
        html=html_content,
        title="Task",
        x=100,
        y=100,
        width=300,
        height=200,
    )


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Return reward based on task completion."""
    global pid

    if pid is None:
        return [0.0]

    # Check if button was clicked via JavaScript state
    clicked = await session.execute_javascript(pid, "window.__clicked")

    return [1.0] if clicked is True else [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Demonstrate the solution."""
    global pid

    if pid is None:
        return

    # Click the button by CSS selector (simulated environments only)
    await session.click_element(pid, "#submit-btn")

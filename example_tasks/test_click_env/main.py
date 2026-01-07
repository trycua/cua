"""Simple click test task for native environments (Linux Docker or Windows QEMU)."""
import cua_bench as cb
from pathlib import Path
import asyncio


@cb.tasks_config(split="train")
def load():
    """Define task variants for each supported OS."""
    os_types = ["linux", "windows"]
    return [
        cb.Task(
            description=f'Click the big blue "Click Me" button to complete the task. ({os_type})',
            metadata={"button_id": "click-me", "os_type": os_type},
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": os_type,
                    "width": 1024,
                    "height": 768,
                }
            }
        )
        for os_type in os_types
    ]


pid = None


@cb.setup_task(split="train")
async def start(task_cfg, session: cb.DesktopSession):
    """Initialize the task - launch a webview with a button."""
    global pid
    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text('utf-8'),
        title="Click Test",
        width=400,
        height=300,
    )
    # Wait for browser to open and render
    await asyncio.sleep(3.0)


@cb.evaluate_task(split="train")
async def evaluate(task_cfg, session: cb.DesktopSession) -> list[float]:
    """Check if the button was clicked.

    For native environments (QEMU), JavaScript execution returns None,
    so we can't verify programmatically. Return 0.5 (uncertain) in that case.
    For simulated environments (Playwright), we can check the JS state.
    """
    global pid
    if pid is None:
        return [0.0]

    clicked = await session.execute_javascript(pid, "window.__clicked")
    if clicked is None:
        # Native environment - can't verify via JS
        # Could implement visual verification here in the future
        return [0.5]  # Uncertain - task ran but can't verify
    return [1.0] if clicked is True else [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg, session: cb.DesktopSession):
    """Oracle solution - click the button.

    For native environments, click at the center of the screen where the
    browser window with the centered button should be.
    """
    global pid
    if pid is None:
        return

    # Try element-based click first (works in simulated environments)
    try:
        await session.click_element(pid, "#click-me")
    except (NotImplementedError, AttributeError):
        # Native environment - use screen coordinates via execute_action
        # The button is centered in a maximized or large browser window
        # Assume it's roughly at screen center
        screen_width = 1024  # From setup_config
        screen_height = 768
        # Click at approximate center of screen
        # Note: Browser chrome adds ~100px at top, so adjust slightly down
        click_action = cb.ClickAction(x=screen_width // 2, y=screen_height // 2 + 50)
        await session.execute_action(click_action)


if __name__ == "__main__":
    cb.interact(__file__)

"""Simple Notepad test task for native Windows environments.

This task tests real Windows application interaction WITHOUT webview/bench_ui:
- Launches Notepad using Windows shell
- Types text
- Saves file
- Evaluates by checking file contents on disk

No webview, no HTML - just real Windows app automation.
"""

import asyncio

import cua_bench as cb


@cb.tasks_config(split="train")
def load():
    """Define Notepad task variants."""
    return [
        cb.Task(
            description='Open Notepad, type "Hello from CUA!", and save as "cua_test.txt" on the Desktop.',
            task_id="notepad-hello",
            metadata={
                "expected_file": r"C:\Users\Docker\Desktop\cua_test.txt",
                "expected_content": "Hello from CUA!",
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "windows",
                    "width": 1920,
                    "height": 1080,
                },
            },
        ),
        cb.Task(
            description='Open Notepad, type "Testing 123", and save as "numbers.txt" on the Desktop.',
            task_id="notepad-numbers",
            metadata={
                "expected_file": r"C:\Users\Docker\Desktop\numbers.txt",
                "expected_content": "Testing 123",
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "windows",
                    "width": 1920,
                    "height": 1080,
                },
            },
        ),
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Setup: Launch Notepad.

    Uses run_command to open Notepad via Windows shell.
    """
    # Launch Notepad using shell command
    await session.run_command("notepad.exe")

    # Wait for Notepad to open and get focus
    await asyncio.sleep(3.0)

    # Click in center of screen to ensure Notepad has focus
    # (Notepad opens centered by default)
    await session.click(960, 540)
    await asyncio.sleep(0.5)

    # Take initial screenshot
    await session.screenshot()


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Evaluate: Check if file exists with correct content.

    Uses file_exists and read_file to verify the task was completed.
    """
    expected_file = task_cfg.metadata.get("expected_file", "")
    expected_content = task_cfg.metadata.get("expected_content", "")

    if not expected_file:
        return [0.0]

    # Check if file exists
    try:
        exists = await session.file_exists(expected_file)
        if not exists:
            print(f"File not found: {expected_file}")
            return [0.0]

        # Read file content
        content = await session.read_file(expected_file)
        content = content.strip() if content else ""

        # Check content
        if content == expected_content:
            print(f"File content matches: '{content}'")
            return [1.0]
        else:
            print(f"Content mismatch: expected '{expected_content}', got '{content}'")
            # Partial credit if file exists but content differs
            return [0.5]

    except Exception as e:
        print(f"Evaluation error: {e}")
        return [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Oracle solution: Type text and save file using keyboard shortcuts.

    This demonstrates pure keyboard/mouse automation without webview:
    1. Notepad should already be open from setup
    2. Type the expected content
    3. Save using Ctrl+S (opens Save As dialog for new file)
    4. Type the full path in the filename field
    5. Press Enter to save
    """
    expected_content = task_cfg.metadata.get("expected_content", "")
    expected_file = task_cfg.metadata.get("expected_file", "")

    # Type the content in Notepad
    await session.type(expected_content)
    await asyncio.sleep(1.0)

    # Take screenshot after typing
    await session.screenshot()

    # Save file: Ctrl+S opens Save As dialog for new/untitled file
    await session.hotkey(["ctrl", "s"])
    await asyncio.sleep(2.0)  # Wait for Save As dialog to open

    # Take screenshot to see Save dialog
    await session.screenshot()

    # The Save As dialog in Windows 11 has the filename field focused at the bottom.
    # We can type the full path directly - Windows will parse it.
    # Clear any existing text first
    await session.hotkey(["ctrl", "a"])
    await asyncio.sleep(0.3)

    # Type the full file path
    # Use the expected_file path directly (e.g., C:\Users\Docker\Desktop\cua_test.txt)
    await session.type(expected_file)
    await asyncio.sleep(0.5)

    # Take screenshot to verify path was typed
    await session.screenshot()

    # Press Enter to save
    await session.key("Return")
    await asyncio.sleep(2.0)

    # Take screenshot after save
    await session.screenshot()

    # Handle potential "file already exists" confirmation dialog
    # Press Left arrow then Enter to select "Yes" if dialog appears
    # Or just press Enter if no dialog
    await session.key("Return")
    await asyncio.sleep(1.0)

    # Final screenshot
    await session.screenshot()


if __name__ == "__main__":
    cb.interact(__file__)

from __future__ import annotations
import time
from bench_ui import launch_window, get_element_rect, execute_javascript
from pathlib import Path
import os

def main():
    os.environ["CUA_BENCH_UI_DEBUG"] = "1"

    # Get the path to the gui folder
    gui_folder = Path(__file__).parent / "gui"
    
    # Launch a window serving the static folder
    pid = launch_window(
        folder=str(gui_folder),
        title="Static Folder Example",
        width=800,
        height=600,
    )
    print(f"Launched window with PID: {pid}")
    print(f"Serving folder: {gui_folder}")

    # Give the window a moment to render
    time.sleep(1.5)

    # Query the client rect of the button element
    rect = get_element_rect(pid, "#testButton", space="window")
    print("Button rect (window space):", rect)

    # Check if button has been clicked
    clicked = execute_javascript(pid, "document.getElementById('testButton').disabled")
    print("Button clicked:", clicked)
    
    # Get the page title
    title = execute_javascript(pid, "document.title")
    print("Page title:", title)


if __name__ == "__main__":
    main()

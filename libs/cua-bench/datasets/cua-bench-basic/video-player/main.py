import cua_bench as cb
from pathlib import Path

# Called once per batch
@cb.tasks_config(split="train")
def load():
    os_types = ["linux"] # ["macos", "win11", "win10"]

    # Different video player interaction scenarios
    video_scenarios = [
        {"action": "pause", "description": "Pause the video"},
        {"action": "play", "description": "Play the video", "initial_state": "paused"},
        {"action": "set_volume", "volume": 75, "description": "Set the volume to 75%"},
        {"action": "set_volume", "volume": 30, "description": "Set the volume to 30%"},
        {"action": "mute", "description": "Mute the video"},
        {"action": "seek", "seek_time": 60, "description": "Seek to 1:00 in the video"},
        {"action": "seek", "seek_time": 120, "description": "Seek to 2:00 in the video"},
    ]

    return [
        cb.Task(
            description=scenario["description"] + ".",
            metadata={
                "action": scenario["action"],
                "volume": scenario.get("volume"),
                "seek_time": scenario.get("seek_time"),
                "initial_state": scenario.get("initial_state", "playing"),
            },
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": os_type,
                    "width": 1024,
                    "height": 768,
                    "background": '#c0c0c0'
                }
            }
        )
        for os_type in os_types
        for scenario in video_scenarios
    ]

# All code below will be running in a separate process per task

pid = None

# Called at start of task
@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Setup steps:
    # 1. Create a webview window with dynamic initial state
    html_template = (Path(__file__).parent / "gui/index.html").read_text('utf-8')
    initial_state = task_cfg.metadata["initial_state"]
    html = html_template.replace("{{INITIAL_STATE}}", initial_state)

    pid = await session.launch_window(
        html=html,
        title="Video Player Task",
        width=600,
        height=450,
    )

# Called at end of task
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    # Evaluation steps:
    # 1. Check the video player state
    player_state = await session.execute_javascript(pid, "window.__playerState")

    if player_state is None:
        return [0.0]

    action = task_cfg.metadata["action"]

    # 2. Verify the action was completed correctly
    if action == "pause":
        return [1.0] if player_state["is_playing"] == False else [0.0]
    elif action == "play":
        return [1.0] if player_state["is_playing"] == True else [0.0]
    elif action == "set_volume":
        target_volume = task_cfg.metadata["volume"]
        # Allow small tolerance for volume (±3)
        volume_matches = abs(player_state["volume"] - target_volume) <= 3
        return [1.0] if volume_matches else [0.0]
    elif action == "mute":
        return [1.0] if player_state["is_muted"] == True else [0.0]
    elif action == "seek":
        target_time = task_cfg.metadata["seek_time"]
        # Allow small tolerance for seek time (±2 seconds)
        time_matches = abs(player_state["current_time"] - target_time) <= 2
        return [1.0] if time_matches else [0.0]

    return [0.0]

# Called after setup_task if run_solution is True
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    action = task_cfg.metadata["action"]

    # Solution steps based on the action
    if action == "pause":
        # Click the play/pause button (which should be showing pause icon)
        await session.click_element(pid, "#play-pause-btn")

    elif action == "play":
        # Click the play/pause button (which should be showing play icon)
        await session.click_element(pid, "#play-pause-btn")

    elif action == "set_volume":
        # Show the volume slider programmatically and get its position
        slider_info = await session.execute_javascript(pid, """
            (function() {
                // Show the volume slider
                const sliderContainer = document.getElementById('volume-slider-container');
                sliderContainer.classList.remove('hidden');

                // Get the slider element and its position
                const slider = document.getElementById('volume-slider');
                const rect = slider.getBoundingClientRect();
                return {
                    x: rect.left + window.screenX,
                    y: rect.top + window.screenY,
                    width: rect.width,
                    height: rect.height
                };
            })()
        """)

        # Calculate target position for vertical slider (0 is at bottom, 100 is at top)
        target_volume = task_cfg.metadata["volume"]
        # For vertical slider, we calculate from bottom to top
        target_x = slider_info["x"] + slider_info["width"] / 2
        target_y = slider_info["y"] + slider_info["height"] - (slider_info["height"] * target_volume / 100)

        # Drag the slider (from current position to target)
        start_x = slider_info["x"] + slider_info["width"] / 2
        start_y = slider_info["y"] + slider_info["height"] / 2

        await session.execute_action(cb.DragAction(
            from_x=start_x,
            from_y=start_y,
            to_x=target_x,
            to_y=target_y,
            duration=0.5
        ))

    elif action == "mute":
        # Click the volume button to toggle mute
        await session.click_element(pid, "#volume-btn")

    elif action == "seek":
        # Get the seek bar info
        seek_bar_info = await session.execute_javascript(pid, """
            (function() {
                const seekBar = document.getElementById('seek-bar');
                const rect = seekBar.getBoundingClientRect();
                return {
                    x: rect.left + window.screenX,
                    y: rect.top + window.screenY,
                    width: rect.width,
                    height: rect.height
                };
            })()
        """)

        # Calculate target position based on seek time (0-225 seconds)
        target_time = task_cfg.metadata["seek_time"]
        target_x = seek_bar_info["x"] + (seek_bar_info["width"] * target_time / 225)
        target_y = seek_bar_info["y"] + seek_bar_info["height"] / 2

        # Drag the seek bar to the target position
        start_x = seek_bar_info["x"] + seek_bar_info["width"] / 2
        start_y = seek_bar_info["y"] + seek_bar_info["height"] / 2

        await session.execute_action(cb.DragAction(
            from_x=start_x,
            from_y=start_y,
            to_x=target_x,
            to_y=target_y,
            duration=0.5
        ))

if __name__ == "__main__":
    cb.interact(__file__)

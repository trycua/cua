import asyncio
import base64
from typing import Any, Dict, List, Optional, Tuple

from ..utils.helpers import CommandExecutor
from .base import (
    BaseAccessibilityHandler,
    BaseAutomationHandler,
    BaseDesktopHandler,
    BaseFileHandler,
    BaseWindowHandler,
)

# Map common key names to Android keycodes
ANDROID_KEY_MAP = {
    "return": "66",
    "enter": "66",
    "backspace": "67",
    "delete": "67",
    "tab": "61",
    "escape": "111",
    "esc": "111",
    "home": "3",
    "back": "4",
    "space": "62",
    "up": "19",
    "down": "20",
    "left": "21",
    "right": "22",
}

adb_exec = CommandExecutor("adb", "-s", "emulator-5554")


class AndroidAccessibilityHandler(BaseAccessibilityHandler):
    """Android accessibility handler using UI Automator."""

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree using uiautomator dump."""
        raise NotImplementedError("get_accessibility_tree not yet implemented for Android")

    async def find_element(
        self, role: Optional[str] = None, title: Optional[str] = None, value: Optional[str] = None
    ) -> Dict[str, Any]:
        """Find an element in the UI hierarchy."""
        raise NotImplementedError("find_element not yet implemented for Android")


class AndroidAutomationHandler(BaseAutomationHandler):
    """Android automation handler using ADB input commands."""

    # Mouse Actions
    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Simulate mouse down (touch down) at position.
        Note: Android doesn't support separate touch down/up via ADB.
        This is a simulated implementation."""
        if x is None or y is None:
            raise ValueError("x and y coordinates are required for mouse_down on Android")
        # Android doesn't support separate touch down/up through ADB
        # We simulate by doing a very short tap
        success, output = await adb_exec.run(
            "shell",
            "input",
            "swipe",
            str(x),
            str(y),
            str(x),
            str(y),
            "100",
            decode=True,
        )
        if success:
            return {}
        else:
            raise RuntimeError(f"Mouse down failed: {output}")

    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Simulate mouse up (touch up) at position.
        Note: Android doesn't support separate touch down/up via ADB.
        This is a simulated implementation."""
        # Android doesn't support separate touch down/up through ADB
        # This is essentially a no-op as mouse_down already completes the touch
        return {}

    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a tap at the specified position."""
        if x is None or y is None:
            raise ValueError("x and y coordinates are required for left_click on Android")

        success, output = await adb_exec.run("shell", "input", "tap", str(x), str(y), decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Tap failed: {output}")

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Simulate right click (long press) at position."""
        if x is None or y is None:
            raise ValueError("x and y coordinates are required for right_click on Android")

        # Long press: swipe with long duration simulates touch and hold
        success, output = await adb_exec.run(
            "shell",
            "input",
            "swipe",
            str(x),
            str(y),
            str(x),
            str(y),
            "1000",
            decode=True,
        )
        if success:
            return {}
        else:
            raise RuntimeError(f"Long press failed: {output}")

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        """Perform a double tap at the specified position."""
        if x is None or y is None:
            raise ValueError("x and y coordinates are required for double_click on Android")

        # Perform two taps in quick succession
        for _ in range(2):
            success, output = await adb_exec.run(
                "shell", "input", "tap", str(x), str(y), decode=True
            )
            if not success:
                raise RuntimeError(f"Double tap failed: {output}")
            await asyncio.sleep(0.1)  # Short delay between taps

        return {}

    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        """Move cursor - not supported on touch devices."""
        raise NotImplementedError("move_cursor not supported on Android (touch-based interface)")

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag from current position to target coordinates.
        Note: Android doesn't track cursor position. This requires the last tap position."""
        # Since Android doesn't track cursor position, we can't implement drag_to properly
        # without knowing the start position. Use drag() with explicit path instead.
        raise NotImplementedError(
            "drag_to not well supported on Android (no cursor tracking). "
            "Use drag(path) with explicit start and end coordinates instead."
        )

    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag along a path of coordinates."""
        if len(path) < 2:
            raise ValueError("Path must contain at least 2 coordinates for drag")

        # Use first and last points for swipe gesture
        start_x, start_y = path[0]
        end_x, end_y = path[-1]

        # Convert duration to milliseconds
        duration_ms = int(duration * 1000)

        success, output = await adb_exec.run(
            "shell",
            "input",
            "swipe",
            str(start_x),
            str(start_y),
            str(end_x),
            str(end_y),
            str(duration_ms),
        )
        if success:
            return {}
        else:
            raise RuntimeError(f"Drag failed: {output}")

    # Keyboard Actions
    async def key_down(self, key: str) -> Dict[str, Any]:
        """Press and hold key - limited support on Android.
        Note: Android doesn't support separate key down/up via ADB."""
        # Android doesn't support key hold through ADB input
        # We simulate by sending the keyevent once
        return await self.press_key(key)

    async def key_up(self, key: str) -> Dict[str, Any]:
        """Release key - limited support on Android.
        Note: Android doesn't support separate key down/up via ADB."""
        # Android doesn't support separate key up through ADB
        # This is essentially a no-op
        return {}

    async def type_text(self, text: str) -> Dict[str, Any]:
        """Type text using Android input method."""
        # Escape special characters for ADB shell
        # Replace spaces with %s (Android's escape for space)
        escaped_text = text.replace(" ", "%s").replace("'", "\\'").replace('"', '\\"')

        success, output = await adb_exec.run("shell", "input", "text", escaped_text, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Type text failed: {output}")

    async def press_key(self, key: str) -> Dict[str, Any]:
        """Press a key using keyevent."""
        keycode = ANDROID_KEY_MAP.get(key.lower(), key)
        success, output = await adb_exec.run("shell", "input", "keyevent", keycode, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Press key failed: {output}")

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        """Press key combination - sends keys in sequence on Android."""
        # Android doesn't support simultaneous key presses via ADB
        # We send keys sequentially
        for key in keys:
            await self.press_key(key)
            await asyncio.sleep(0.05)  # Small delay between keys
        return {}

    # Scrolling Actions
    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        """Scroll by x and y amounts."""
        # Get screen size to calculate swipe positions
        screen_size = await self.get_screen_size()
        width, height = screen_size["width"], screen_size["height"]

        # Use center of screen as starting point
        center_x, center_y = width // 2, height // 2

        # Calculate end points (negative y means scroll down, positive means scroll up)
        end_x = center_x + x
        end_y = center_y - y  # Inverted because swipe up scrolls content down

        success, output = await adb_exec.run(
            "shell",
            "input",
            "swipe",
            str(center_x),
            str(center_y),
            str(end_x),
            str(end_y),
            "300",
            decode=True,
        )
        if success:
            return {}
        else:
            raise RuntimeError(f"Scroll failed: {output}")

    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll down by specified number of clicks."""
        # Get screen size
        screen_size = await self.get_screen_size()
        width, height = screen_size["width"], screen_size["height"]

        # Swipe up to scroll content down
        center_x = width // 2
        start_y = int(height * 0.7)
        end_y = int(height * 0.3)

        for _ in range(clicks):
            success, output = await adb_exec.run(
                "shell",
                "input",
                "swipe",
                str(center_x),
                str(start_y),
                str(center_x),
                str(end_y),
                "300",
                decode=True,
            )
            if not success:
                raise RuntimeError(f"Scroll down failed: {output}")
            await asyncio.sleep(0.1)  # Small delay between scrolls

        return {}

    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll up by specified number of clicks."""
        # Get screen size
        screen_size = await self.get_screen_size()
        width, height = screen_size["width"], screen_size["height"]

        # Swipe down to scroll content up
        center_x = width // 2
        start_y = int(height * 0.3)
        end_y = int(height * 0.7)

        for _ in range(clicks):
            success, output = await adb_exec.run(
                "shell",
                "input",
                "swipe",
                str(center_x),
                str(start_y),
                str(center_x),
                str(end_y),
                "300",
                decode=True,
            )
            if not success:
                raise RuntimeError(f"Scroll up failed: {output}")
            await asyncio.sleep(0.1)  # Small delay between scrolls

        return {}

    # Screen Actions
    async def screenshot(self) -> Dict[str, Any]:
        """Take a screenshot and return base64 encoded image."""
        success, output = await adb_exec.run("shell", "screencap", "-p")
        if success and output:
            image_b64 = base64.b64encode(output).decode("utf-8")
            return {"image_data": image_b64}
        else:
            raise RuntimeError(f"Screenshot failed: {output.decode('utf-8')}")

    async def get_screen_size(self) -> Dict[str, Any]:
        """Get the screen size of the Android device."""
        success, output = await adb_exec.run("shell", "wm", "size", decode=True)
        if success and "x" in output:
            # Parse "Physical size: 1080x1920"
            size_str = output.split(":")[-1].strip()
            width, height = map(int, size_str.split("x"))
            return {"width": width, "height": height}
        else:
            raise RuntimeError(f"Failed to get screen size: {output}")

    async def get_cursor_position(self) -> Dict[str, Any]:
        """Get cursor position - not supported on touch devices."""
        raise NotImplementedError(
            "get_cursor_position not supported on Android (touch-based interface)"
        )

    # Clipboard Actions
    async def copy_to_clipboard(self) -> Dict[str, Any]:
        """Get clipboard content."""
        # Android 10+ supports clipboard via cmd
        success, output = await adb_exec.run("shell", "cmd", "clipboard", "get-text", decode=True)
        if success:
            return {"text": output.strip()}
        else:
            raise RuntimeError(f"Failed to get clipboard: {output}")

    async def set_clipboard(self, text: str) -> Dict[str, Any]:
        """Set clipboard content."""
        # Android 10+ supports clipboard via cmd
        success, output = await adb_exec.run(
            "shell", "cmd", "clipboard", "set-text", text, decode=True
        )
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to set clipboard: {output}")

    # Other
    async def run_command(self, command: str) -> Dict[str, Any]:
        """Run a shell command on Android device."""
        success, output = await adb_exec.run("shell", command, decode=True)
        return {"output": output, "success": success}


class AndroidFileHandler(BaseFileHandler):
    """Android file handler using ADB shell commands."""

    async def file_exists(self, path: str) -> Dict[str, Any]:
        """Check if a file exists."""
        success, output = await adb_exec.run(
            "shell", f"test -f '{path}' && echo 'yes' || echo 'no'", decode=True
        )
        exists = success and output.strip() == "yes"
        return {"exists": exists}

    async def directory_exists(self, path: str) -> Dict[str, Any]:
        """Check if a directory exists."""
        success, output = await adb_exec.run(
            "shell", f"test -d '{path}' && echo 'yes' || echo 'no'", decode=True
        )
        exists = success and output.strip() == "yes"
        return {"exists": exists}

    async def list_dir(self, path: str) -> Dict[str, Any]:
        """List directory contents."""
        success, output = await adb_exec.run("shell", "ls", "-la", path, decode=True)
        if success:
            # Parse ls -la output
            lines = output.strip().split("\n")
            entries = []
            for line in lines[1:]:  # Skip "total" line
                if line:
                    parts = line.split()
                    if len(parts) >= 9:
                        name = " ".join(parts[8:])
                        if name not in [".", ".."]:
                            entries.append(
                                {
                                    "name": name,
                                    "is_dir": parts[0].startswith("d"),
                                    "size": int(parts[4]) if parts[4].isdigit() else 0,
                                }
                            )
            return {"entries": entries}
        else:
            raise RuntimeError(f"Failed to list directory: {output}")

    async def read_text(self, path: str) -> Dict[str, Any]:
        """Read text file contents."""
        success, output = await adb_exec.run("shell", "cat", path, decode=True)
        if success:
            return {"content": output}
        else:
            raise RuntimeError(f"Failed to read file: {output}")

    async def write_text(self, path: str, content: str) -> Dict[str, Any]:
        """Write text to file."""
        # Escape single quotes in content
        escaped_content = content.replace("'", "'\"'\"'")
        success, output = await adb_exec.run(
            "shell", f"printf '%s' '{escaped_content}' > '{path}'", decode=True
        )
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to write file: {output}")

    async def write_bytes(self, path: str, content_b64: str) -> Dict[str, Any]:
        """Write binary content to file."""
        # Decode base64 and write to temp file, then push to device
        import os
        import tempfile

        content_bytes = base64.b64decode(content_b64)

        # Create temp file
        with tempfile.NamedTemporaryFile(delete=False) as tmp:
            tmp.write(content_bytes)
            tmp_path = tmp.name

        try:
            # Push file to device
            success, output = await adb_exec.run("push", tmp_path, path, decode=True)
            if success:
                return {}
            else:
                raise RuntimeError(f"Failed to write bytes: {output}")
        finally:
            os.unlink(tmp_path)

    async def delete_file(self, path: str) -> Dict[str, Any]:
        """Delete a file."""
        success, output = await adb_exec.run("shell", "rm", "-f", path, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to delete file: {output}")

    async def create_dir(self, path: str) -> Dict[str, Any]:
        """Create a directory."""
        success, output = await adb_exec.run("shell", "mkdir", "-p", path, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to create directory: {output}")

    async def delete_dir(self, path: str) -> Dict[str, Any]:
        """Delete a directory."""
        success, output = await adb_exec.run("shell", "rm", "-rf", path, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to delete directory: {output}")

    async def read_bytes(
        self, path: str, offset: int = 0, length: Optional[int] = None
    ) -> Dict[str, Any]:
        """Read binary file contents."""
        # Pull file from device and read bytes
        import os
        import tempfile

        with tempfile.NamedTemporaryFile(delete=False) as tmp:
            tmp_path = tmp.name

        try:
            # Pull file from device
            success, output = await adb_exec.run("pull", path, tmp_path, decode=True)
            if not success:
                raise RuntimeError(f"Failed to pull file: {output}")

            # Read bytes from temp file
            with open(tmp_path, "rb") as f:
                f.seek(offset)
                if length is not None:
                    content_bytes = f.read(length)
                else:
                    content_bytes = f.read()

            content_b64 = base64.b64encode(content_bytes).decode("utf-8")
            return {"content": content_b64}
        finally:
            if os.path.exists(tmp_path):
                os.unlink(tmp_path)

    async def get_file_size(self, path: str) -> Dict[str, Any]:
        """Get file size in bytes."""
        success, output = await adb_exec.run("shell", f"wc -c < '{path}'", decode=True)
        if success:
            try:
                size = int(output.strip())
                return {"size": size}
            except ValueError:
                raise RuntimeError(f"Failed to parse file size: {output}")
        else:
            raise RuntimeError(f"Failed to get file size: {output}")


class AndroidWindowHandler(BaseWindowHandler):
    """Android window/app handler using activity manager."""

    async def open(self, target: str) -> Dict[str, Any]:
        """Open a URL or file with default app."""
        # Use ACTION_VIEW intent to open URL or file
        success, output = await adb_exec.run(
            "shell", "am", "start", "-a", "android.intent.action.VIEW", "-d", target, decode=True
        )
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to open target: {output}")

    async def launch(self, app: str, args: Optional[List[str]] = None) -> Dict[str, Any]:
        """Launch an Android app by package name or activity."""
        # If app contains '/', it's package/activity, otherwise just package
        if "/" in app:
            cmd = ["shell", "am", "start", "-n", app]
        else:
            # Launch main activity for package
            cmd = ["shell", "monkey", "-p", app, "-c", "android.intent.category.LAUNCHER", "1"]

        if args:
            # Add extras if provided
            for arg in args:
                if "=" in arg:
                    key, value = arg.split("=", 1)
                    cmd.extend(["--es", key, value])

        success, output = await adb_exec.run(*cmd, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to launch app: {output}")

    async def get_current_window_id(self) -> Dict[str, Any]:
        """Get the currently focused activity."""
        import logging

        logger = logging.getLogger(__name__)

        success, output = await adb_exec.run("shell", "dumpsys", "window", decode=True)
        if success:
            # Parse mCurrentFocus line
            for line in output.split("\n"):
                if "mCurrentFocus" in line:
                    logger.info(f"Found mCurrentFocus line: {line}")
                    # Example: mCurrentFocus=Window{abc123 u0 com.android.launcher3/com.android.launcher3.Launcher}
                    import re

                    match = re.search(r"([a-zA-Z0-9._]+/[a-zA-Z0-9._$]+)\}", line)
                    if match:
                        window_id = match.group(1)
                        logger.info(f"Extracted window_id: {window_id}")
                        return {"window_id": window_id}
                    else:
                        logger.warning(f"Regex did not match line: {line}")
            logger.warning("No mCurrentFocus line found in dumpsys output")
            return {"window_id": "unknown"}
        else:
            raise RuntimeError(f"Failed to get current window: {output}")

    async def get_application_windows(self, app: str) -> Dict[str, Any]:
        """Get activities for an app."""
        # List all activities in the package
        success, output = await adb_exec.run("shell", "dumpsys", "package", app, decode=True)
        if success:
            activities = []
            in_activity_section = False
            for line in output.split("\n"):
                if "Activity Resolver Table:" in line:
                    in_activity_section = True
                elif in_activity_section and app in line:
                    import re

                    match = re.search(r"([a-z0-9.]+/[a-z0-9.]+)", line)
                    if match:
                        activities.append(match.group(1))

            return {"windows": activities}
        else:
            raise RuntimeError(f"Failed to get application windows: {output}")

    async def get_window_name(self, window_id: str) -> Dict[str, Any]:
        """Get the name of an activity."""
        # window_id is in format package/activity
        if "/" in str(window_id):
            activity_name = str(window_id).split("/")[-1]
            return {"name": activity_name}
        else:
            return {"name": str(window_id)}

    async def get_window_size(self, window_id: str | int) -> Dict[str, Any]:
        """Get window size (returns screen size on Android)."""
        # Android apps are typically fullscreen, return screen size
        success, output = await adb_exec.run("shell", "wm", "size", decode=True)
        if success and "x" in output:
            size_str = output.split(":")[-1].strip()
            width, height = map(int, size_str.split("x"))
            return {"width": width, "height": height}
        else:
            raise RuntimeError(f"Failed to get window size: {output}")

    async def activate_window(self, window_id: str | int) -> Dict[str, Any]:
        """Bring an app to foreground."""
        # window_id should be package/activity format
        window_str = str(window_id)
        success, output = await adb_exec.run("shell", "am", "start", "-n", window_str, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to activate window: {output}")

    async def close_window(self, window_id: str | int) -> Dict[str, Any]:
        """Force stop an app."""
        # Extract package name from window_id (package/activity format)
        window_str = str(window_id)
        package = window_str.split("/")[0] if "/" in window_str else window_str

        success, output = await adb_exec.run("shell", "am", "force-stop", package, decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to close window: {output}")

    async def get_window_position(self, window_id: str | int) -> Dict[str, Any]:
        """Get window position - not supported on Android."""
        raise NotImplementedError(
            "get_window_position not supported on Android (no windowing system)"
        )

    async def set_window_size(
        self, window_id: str | int, width: int, height: int
    ) -> Dict[str, Any]:
        """Set window size - not supported on Android."""
        raise NotImplementedError("set_window_size not supported on Android (apps are fullscreen)")

    async def set_window_position(self, window_id: str | int, x: int, y: int) -> Dict[str, Any]:
        """Set window position - not supported on Android."""
        raise NotImplementedError(
            "set_window_position not supported on Android (no windowing system)"
        )

    async def maximize_window(self, window_id: str | int) -> Dict[str, Any]:
        """Maximize window - not supported on Android."""
        raise NotImplementedError(
            "maximize_window not supported on Android (apps always fullscreen)"
        )

    async def minimize_window(self, window_id: str | int) -> Dict[str, Any]:
        """Minimize window (send to background)."""
        # Press HOME key to minimize current app
        success, output = await adb_exec.run("shell", "input", "keyevent", "3", decode=True)
        if success:
            return {}
        else:
            raise RuntimeError(f"Failed to minimize window: {output}")


class AndroidDesktopHandler(BaseDesktopHandler):
    """Android desktop handler - minimal implementation."""

    async def get_desktop_environment(self) -> Dict[str, Any]:
        """Get desktop environment name."""
        return {"desktop_environment": "android"}

    async def set_wallpaper(self, path: str):
        """
        Set the wallpaper using our custom helper APK.

        Args:
            path: Absolute path to image on device (e.g. /sdcard/Pictures/wall.jpg)
        """
        # Copy file to /data/local/tmp where all apps can read it
        # (/sdcard uses FUSE with restrictive permissions that chmod can't change)
        import os

        temp_path = f"/data/local/tmp/wallpaper_{os.path.basename(path)}"

        # Copy to temp location with world-readable permissions
        copy_success, _ = await adb_exec.run("shell", "cp", path, temp_path, decode=True)
        if not copy_success:
            raise RuntimeError(f"Failed to copy file to temp location: {path}")

        # Make temp file readable
        await adb_exec.run("shell", "chmod", "644", temp_path, decode=True)

        package = "com.example.cua.wallpaper"
        component = f"{package}/.SetWallpaperActivity"

        success, output = await adb_exec.run(
            "shell",
            "am",
            "start",
            "-n",
            component,
            "-a",
            "com.example.cua.wallpaper.SET_WALLPAPER",
            "-e",
            "path",
            temp_path,
            "-e",
            "target",
            "home",
            decode=True,
        )

        if success:
            # Give it a moment to set the wallpaper
            await asyncio.sleep(1)
            # Clean up temp file
            await adb_exec.run("shell", "rm", temp_path, decode=True)
            return {}

        # Clean up on failure too
        await adb_exec.run("shell", "rm", temp_path, decode=True)
        raise RuntimeError(f"Failed to set wallpaper: {output}")

"""Base interface for computer control."""

from abc import ABC, abstractmethod
from typing import Any, Dict, List, Optional, Tuple

from ..logger import Logger, LogLevel
from .models import CommandResult, MouseButton


class BaseComputerInterface(ABC):
    """Base class for computer control interfaces."""

    def __init__(
        self,
        ip_address: str,
        username: str = "lume",
        password: str = "lume",
        api_key: Optional[str] = None,
        vm_name: Optional[str] = None,
    ):
        """Initialize interface.

        Args:
            ip_address: IP address of the computer to control
            username: Username for authentication
            password: Password for authentication
            api_key: Optional API key for cloud authentication
            vm_name: Optional VM name for cloud authentication
        """
        self.ip_address = ip_address
        self.username = username
        self.password = password
        self.api_key = api_key
        self.vm_name = vm_name
        self.logger = Logger("cua.interface", LogLevel.NORMAL)

        # Optional default delay time between commands (in seconds)
        self.delay: float = 0.0

    @abstractmethod
    async def wait_for_ready(self, timeout: int = 60) -> None:
        """Wait for interface to be ready.

        Args:
            timeout: Maximum time to wait in seconds

        Raises:
            TimeoutError: If interface is not ready within timeout
        """
        pass

    @abstractmethod
    def close(self) -> None:
        """Close the interface connection."""
        pass

    def force_close(self) -> None:
        """Force close the interface connection.

        By default, this just calls close(), but subclasses can override
        to provide more forceful cleanup.
        """
        self.close()

    # Mouse Actions
    @abstractmethod
    async def mouse_down(
        self,
        x: Optional[int] = None,
        y: Optional[int] = None,
        button: "MouseButton" = "left",
        delay: Optional[float] = None,
    ) -> None:
        """Press and hold a mouse button.

        Args:
            x: X coordinate to press at. If None, uses current cursor position.
            y: Y coordinate to press at. If None, uses current cursor position.
            button: Mouse button to press ('left', 'middle', 'right').
            delay: Optional delay in seconds after the action
        """
        pass

    @abstractmethod
    async def mouse_up(
        self,
        x: Optional[int] = None,
        y: Optional[int] = None,
        button: "MouseButton" = "left",
        delay: Optional[float] = None,
    ) -> None:
        """Release a mouse button.

        Args:
            x: X coordinate to release at. If None, uses current cursor position.
            y: Y coordinate to release at. If None, uses current cursor position.
            button: Mouse button to release ('left', 'middle', 'right').
            delay: Optional delay in seconds after the action
        """
        pass

    @abstractmethod
    async def left_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        """Perform a left mouse button click.

        Args:
            x: X coordinate to click at. If None, uses current cursor position.
            y: Y coordinate to click at. If None, uses current cursor position.
            delay: Optional delay in seconds after the action
        """
        pass

    @abstractmethod
    async def right_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        """Perform a right mouse button click.

        Args:
            x: X coordinate to click at. If None, uses current cursor position.
            y: Y coordinate to click at. If None, uses current cursor position.
            delay: Optional delay in seconds after the action
        """
        pass

    @abstractmethod
    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        """Perform a double left mouse button click.

        Args:
            x: X coordinate to double-click at. If None, uses current cursor position.
            y: Y coordinate to double-click at. If None, uses current cursor position.
            delay: Optional delay in seconds after the action
        """
        pass

    @abstractmethod
    async def move_cursor(self, x: int, y: int, delay: Optional[float] = None) -> None:
        """Move the cursor to the specified screen coordinates.

        Args:
            x: X coordinate to move cursor to.
            y: Y coordinate to move cursor to.
            delay: Optional delay in seconds after the action
        """
        pass

    @abstractmethod
    async def drag_to(
        self,
        x: int,
        y: int,
        button: str = "left",
        duration: float = 0.5,
        delay: Optional[float] = None,
    ) -> None:
        """Drag from current position to specified coordinates.

        Args:
            x: The x coordinate to drag to
            y: The y coordinate to drag to
            button: The mouse button to use ('left', 'middle', 'right')
            duration: How long the drag should take in seconds
            delay: Optional delay in seconds after the action
        """
        pass

    @abstractmethod
    async def drag(
        self,
        path: List[Tuple[int, int]],
        button: str = "left",
        duration: float = 0.5,
        delay: Optional[float] = None,
    ) -> None:
        """Drag the cursor along a path of coordinates.

        Args:
            path: List of (x, y) coordinate tuples defining the drag path
            button: The mouse button to use ('left', 'middle', 'right')
            duration: Total time in seconds that the drag operation should take
            delay: Optional delay in seconds after the action
        """
        pass

    # Keyboard Actions
    @abstractmethod
    async def key_down(self, key: str, delay: Optional[float] = None) -> None:
        """Press and hold a key.

        Args:
            key: The key to press and hold (e.g., 'a', 'shift', 'ctrl').
            delay: Optional delay in seconds after the action.
        """
        pass

    @abstractmethod
    async def key_up(self, key: str, delay: Optional[float] = None) -> None:
        """Release a previously pressed key.

        Args:
            key: The key to release (e.g., 'a', 'shift', 'ctrl').
            delay: Optional delay in seconds after the action.
        """
        pass

    @abstractmethod
    async def type_text(self, text: str, delay: Optional[float] = None) -> None:
        """Type the specified text string.

        Args:
            text: The text string to type.
            delay: Optional delay in seconds after the action.
        """
        pass

    @abstractmethod
    async def press_key(self, key: str, delay: Optional[float] = None) -> None:
        """Press and release a single key.

        Args:
            key: The key to press (e.g., 'a', 'enter', 'escape').
            delay: Optional delay in seconds after the action.
        """
        pass

    @abstractmethod
    async def hotkey(self, *keys: str, delay: Optional[float] = None) -> None:
        """Press multiple keys simultaneously (keyboard shortcut).

        Args:
            *keys: Variable number of keys to press together (e.g., 'ctrl', 'c').
            delay: Optional delay in seconds after the action.
        """
        pass

    # Scrolling Actions
    @abstractmethod
    async def scroll(self, x: int, y: int, delay: Optional[float] = None) -> None:
        """Scroll the mouse wheel by specified amounts.

        Args:
            x: Horizontal scroll amount (positive = right, negative = left).
            y: Vertical scroll amount (positive = up, negative = down).
            delay: Optional delay in seconds after the action.
        """
        pass

    @abstractmethod
    async def scroll_down(self, clicks: int = 1, delay: Optional[float] = None) -> None:
        """Scroll down by the specified number of clicks.

        Args:
            clicks: Number of scroll clicks to perform downward.
            delay: Optional delay in seconds after the action.
        """
        pass

    @abstractmethod
    async def scroll_up(self, clicks: int = 1, delay: Optional[float] = None) -> None:
        """Scroll up by the specified number of clicks.

        Args:
            clicks: Number of scroll clicks to perform upward.
            delay: Optional delay in seconds after the action.
        """
        pass

    # Screen Actions
    @abstractmethod
    async def screenshot(self) -> bytes:
        """Take a screenshot.

        Returns:
            Raw bytes of the screenshot image
        """
        pass

    @abstractmethod
    async def get_screen_size(self) -> Dict[str, int]:
        """Get the screen dimensions.

        Returns:
            Dict with 'width' and 'height' keys
        """
        pass

    @abstractmethod
    async def get_cursor_position(self) -> Dict[str, int]:
        """Get the current cursor position on screen.

        Returns:
            Dict with 'x' and 'y' keys containing cursor coordinates.
        """
        pass

    # Clipboard Actions
    @abstractmethod
    async def copy_to_clipboard(self) -> str:
        """Get the current clipboard content.

        Returns:
            The text content currently stored in the clipboard.
        """
        pass

    @abstractmethod
    async def set_clipboard(self, text: str) -> None:
        """Set the clipboard content to the specified text.

        Args:
            text: The text to store in the clipboard.
        """
        pass

    # File System Actions
    @abstractmethod
    async def file_exists(self, path: str) -> bool:
        """Check if a file exists at the specified path.

        Args:
            path: The file path to check.

        Returns:
            True if the file exists, False otherwise.
        """
        pass

    @abstractmethod
    async def directory_exists(self, path: str) -> bool:
        """Check if a directory exists at the specified path.

        Args:
            path: The directory path to check.

        Returns:
            True if the directory exists, False otherwise.
        """
        pass

    @abstractmethod
    async def list_dir(self, path: str) -> List[str]:
        """List the contents of a directory.

        Args:
            path: The directory path to list.

        Returns:
            List of file and directory names in the specified directory.
        """
        pass

    @abstractmethod
    async def read_text(self, path: str) -> str:
        """Read the text contents of a file.

        Args:
            path: The file path to read from.

        Returns:
            The text content of the file.
        """
        pass

    @abstractmethod
    async def write_text(self, path: str, content: str) -> None:
        """Write text content to a file.

        Args:
            path: The file path to write to.
            content: The text content to write.
        """
        pass

    @abstractmethod
    async def read_bytes(self, path: str, offset: int = 0, length: Optional[int] = None) -> bytes:
        """Read file binary contents with optional seeking support.

        Args:
            path: Path to the file
            offset: Byte offset to start reading from (default: 0)
            length: Number of bytes to read (default: None for entire file)
        """
        pass

    @abstractmethod
    async def write_bytes(self, path: str, content: bytes) -> None:
        """Write binary content to a file.

        Args:
            path: The file path to write to.
            content: The binary content to write.
        """
        pass

    @abstractmethod
    async def delete_file(self, path: str) -> None:
        """Delete a file at the specified path.

        Args:
            path: The file path to delete.
        """
        pass

    @abstractmethod
    async def create_dir(self, path: str) -> None:
        """Create a directory at the specified path.

        Args:
            path: The directory path to create.
        """
        pass

    @abstractmethod
    async def delete_dir(self, path: str) -> None:
        """Delete a directory at the specified path.

        Args:
            path: The directory path to delete.
        """
        pass

    @abstractmethod
    async def get_file_size(self, path: str) -> int:
        """Get the size of a file in bytes.

        Args:
            path: The file path to get the size of.

        Returns:
            The size of the file in bytes.
        """
        pass

    # Desktop actions
    @abstractmethod
    async def get_desktop_environment(self) -> str:
        """Get the current desktop environment.

        Returns:
            The name of the current desktop environment.
        """
        pass

    @abstractmethod
    async def set_wallpaper(self, path: str) -> None:
        """Set the desktop wallpaper to the specified path.

        Args:
            path: The file path to set as wallpaper
        """
        pass

    # Window management
    @abstractmethod
    async def open(self, target: str) -> None:
        """Open a target using the system's default handler.

        Typically opens files, folders, or URLs with the associated application.

        Args:
            target: The file path, folder path, or URL to open.
        """
        pass

    @abstractmethod
    async def launch(self, app: str, args: List[str] | None = None) -> Optional[int]:
        """Launch an application with optional arguments.

        Args:
            app: The application executable or bundle identifier.
            args: Optional list of arguments to pass to the application.

        Returns:
            Optional process ID (PID) of the launched application if available, otherwise None.
        """
        pass

    @abstractmethod
    async def get_current_window_id(self) -> int | str:
        """Get the identifier of the currently active/focused window.

        Returns:
            A window identifier that can be used with other window management methods.
        """
        pass

    @abstractmethod
    async def get_application_windows(self, app: str) -> List[int | str]:
        """Get all window identifiers for a specific application.

        Args:
            app: The application name, executable, or identifier to query.

        Returns:
            A list of window identifiers belonging to the specified application.
        """
        pass

    @abstractmethod
    async def get_window_name(self, window_id: int | str) -> str:
        """Get the title/name of a window.

        Args:
            window_id: The window identifier.

        Returns:
            The window's title or name string.
        """
        pass

    @abstractmethod
    async def get_window_size(self, window_id: int | str) -> tuple[int, int]:
        """Get the size of a window in pixels.

        Args:
            window_id: The window identifier.

        Returns:
            A tuple of (width, height) representing the window size in pixels.
        """
        pass

    @abstractmethod
    async def get_window_position(self, window_id: int | str) -> tuple[int, int]:
        """Get the screen position of a window.

        Args:
            window_id: The window identifier.

        Returns:
            A tuple of (x, y) representing the window's top-left corner in screen coordinates.
        """
        pass

    @abstractmethod
    async def set_window_size(self, window_id: int | str, width: int, height: int) -> None:
        """Set the size of a window in pixels.

        Args:
            window_id: The window identifier.
            width: Desired width in pixels.
            height: Desired height in pixels.
        """
        pass

    @abstractmethod
    async def set_window_position(self, window_id: int | str, x: int, y: int) -> None:
        """Move a window to a specific position on the screen.

        Args:
            window_id: The window identifier.
            x: X coordinate for the window's top-left corner.
            y: Y coordinate for the window's top-left corner.
        """
        pass

    @abstractmethod
    async def maximize_window(self, window_id: int | str) -> None:
        """Maximize a window.

        Args:
            window_id: The window identifier.
        """
        pass

    @abstractmethod
    async def minimize_window(self, window_id: int | str) -> None:
        """Minimize a window.

        Args:
            window_id: The window identifier.
        """
        pass

    @abstractmethod
    async def activate_window(self, window_id: int | str) -> None:
        """Bring a window to the foreground and focus it.

        Args:
            window_id: The window identifier.
        """
        pass

    @abstractmethod
    async def close_window(self, window_id: int | str) -> None:
        """Close a window.

        Args:
            window_id: The window identifier.
        """
        pass

    # Convenience aliases
    async def get_window_title(self, window_id: int | str) -> str:
        """Convenience alias for get_window_name().

        Args:
            window_id: The window identifier.

        Returns:
            The window's title or name string.
        """
        return await self.get_window_name(window_id)

    async def window_size(self, window_id: int | str) -> tuple[int, int]:
        """Convenience alias for get_window_size().

        Args:
            window_id: The window identifier.

        Returns:
            A tuple of (width, height) representing the window size in pixels.
        """
        return await self.get_window_size(window_id)

    # Shell actions
    @abstractmethod
    async def run_command(self, command: str) -> CommandResult:
        """Run shell command and return structured result.

        Executes a shell command using subprocess.run with shell=True and check=False.
        The command is run in the target environment and captures both stdout and stderr.

        Args:
            command (str): The shell command to execute

        Returns:
            CommandResult: A structured result containing:
                - stdout (str): Standard output from the command
                - stderr (str): Standard error from the command
                - returncode (int): Exit code from the command (0 indicates success)

        Raises:
            RuntimeError: If the command execution fails at the system level

        Example:
            result = await interface.run_command("ls -la")
            if result.returncode == 0:
                print(f"Output: {result.stdout}")
            else:
                print(f"Error: {result.stderr}, Exit code: {result.returncode}")
        """
        pass

    # Accessibility Actions
    @abstractmethod
    async def get_accessibility_tree(self) -> Dict:
        """Get the accessibility tree of the current screen.

        Returns:
            Dict containing the hierarchical accessibility information of screen elements.
        """
        pass

    @abstractmethod
    async def to_screen_coordinates(self, x: float, y: float) -> tuple[float, float]:
        """Convert screenshot coordinates to screen coordinates.

        Args:
            x: X coordinate in screenshot space
            y: Y coordinate in screenshot space

        Returns:
            tuple[float, float]: (x, y) coordinates in screen space
        """
        pass

    @abstractmethod
    async def to_screenshot_coordinates(self, x: float, y: float) -> tuple[float, float]:
        """Convert screen coordinates to screenshot coordinates.

        Args:
            x: X coordinate in screen space
            y: Y coordinate in screen space

        Returns:
            tuple[float, float]: (x, y) coordinates in screenshot space
        """
        pass

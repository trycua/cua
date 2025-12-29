from abc import ABC, abstractmethod
from typing import Any, Dict, List, Optional, Tuple


class BaseAccessibilityHandler(ABC):
    """Abstract base class for OS-specific accessibility handlers."""

    @abstractmethod
    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree of the current window."""
        pass

    @abstractmethod
    async def find_element(
        self, role: Optional[str] = None, title: Optional[str] = None, value: Optional[str] = None
    ) -> Dict[str, Any]:
        """Find an element in the accessibility tree by criteria."""
        pass


class BaseFileHandler(ABC):
    """Abstract base class for OS-specific file handlers."""

    @abstractmethod
    async def file_exists(self, path: str) -> Dict[str, Any]:
        """Check if a file exists at the specified path."""
        pass

    @abstractmethod
    async def directory_exists(self, path: str) -> Dict[str, Any]:
        """Check if a directory exists at the specified path."""
        pass

    @abstractmethod
    async def list_dir(self, path: str) -> Dict[str, Any]:
        """List the contents of a directory."""
        pass

    @abstractmethod
    async def read_text(self, path: str) -> Dict[str, Any]:
        """Read the text contents of a file."""
        pass

    @abstractmethod
    async def write_text(self, path: str, content: str) -> Dict[str, Any]:
        """Write text content to a file."""
        pass

    @abstractmethod
    async def write_bytes(self, path: str, content_b64: str) -> Dict[str, Any]:
        """Write binary content to a file. Sent over the websocket as a base64 string."""
        pass

    @abstractmethod
    async def delete_file(self, path: str) -> Dict[str, Any]:
        """Delete a file."""
        pass

    @abstractmethod
    async def create_dir(self, path: str) -> Dict[str, Any]:
        """Create a directory."""
        pass

    @abstractmethod
    async def delete_dir(self, path: str) -> Dict[str, Any]:
        """Delete a directory."""
        pass

    @abstractmethod
    async def read_bytes(
        self, path: str, offset: int = 0, length: Optional[int] = None
    ) -> Dict[str, Any]:
        """Read the binary contents of a file. Sent over the websocket as a base64 string.

        Args:
            path: Path to the file
            offset: Byte offset to start reading from (default: 0)
            length: Number of bytes to read (default: None for entire file)
        """
        pass

    @abstractmethod
    async def get_file_size(self, path: str) -> Dict[str, Any]:
        """Get the size of a file in bytes."""
        pass


class BaseDesktopHandler(ABC):
    """Abstract base class for OS-specific desktop handlers.

    Categories:
    - Wallpaper Actions: Methods for wallpaper operations
    - Desktop shortcut actions: Methods for managing desktop shortcuts
    """

    # Wallpaper Actions
    @abstractmethod
    async def get_desktop_environment(self) -> Dict[str, Any]:
        """Get the current desktop environment name."""
        pass

    @abstractmethod
    async def set_wallpaper(self, path: str) -> Dict[str, Any]:
        """Set the desktop wallpaper to the file at path."""
        pass


class BaseWindowHandler(ABC):
    """Abstract class for OS-specific window management handlers.

    Categories:
    - Window Management: Methods for application/window control
    """

    # Window Management
    @abstractmethod
    async def open(self, target: str) -> Dict[str, Any]:
        """Open a file or URL with the default application."""
        pass

    @abstractmethod
    async def launch(self, app: str, args: Optional[List[str]] = None) -> Dict[str, Any]:
        """Launch an application with optional arguments."""
        pass

    @abstractmethod
    async def get_current_window_id(self) -> Dict[str, Any]:
        """Get the currently active window ID."""
        pass

    @abstractmethod
    async def get_application_windows(self, app: str) -> Dict[str, Any]:
        """Get windows belonging to an application (by name or bundle)."""
        pass

    @abstractmethod
    async def get_window_name(self, window_id: str) -> Dict[str, Any]:
        """Get the title/name of a window by ID."""
        pass

    @abstractmethod
    async def get_window_size(self, window_id: str | int) -> Dict[str, Any]:
        """Get the size of a window by ID as {width, height}."""
        pass

    @abstractmethod
    async def activate_window(self, window_id: str | int) -> Dict[str, Any]:
        """Bring a window to the foreground by ID."""
        pass

    @abstractmethod
    async def close_window(self, window_id: str | int) -> Dict[str, Any]:
        """Close a window by ID."""
        pass

    @abstractmethod
    async def get_window_position(self, window_id: str | int) -> Dict[str, Any]:
        """Get the top-left position of a window as {x, y}."""
        pass

    @abstractmethod
    async def set_window_size(
        self, window_id: str | int, width: int, height: int
    ) -> Dict[str, Any]:
        """Set the size of a window by ID."""
        pass

    @abstractmethod
    async def set_window_position(self, window_id: str | int, x: int, y: int) -> Dict[str, Any]:
        """Set the position of a window by ID."""
        pass

    @abstractmethod
    async def maximize_window(self, window_id: str | int) -> Dict[str, Any]:
        """Maximize a window by ID."""
        pass

    @abstractmethod
    async def minimize_window(self, window_id: str | int) -> Dict[str, Any]:
        """Minimize a window by ID."""
        pass


class BaseAutomationHandler(ABC):
    """Abstract base class for OS-specific automation handlers.

    Categories:
    - Mouse Actions: Methods for mouse control
    - Keyboard Actions: Methods for keyboard input
    - Scrolling Actions: Methods for scrolling
    - Screen Actions: Methods for screen interaction
    - Clipboard Actions: Methods for clipboard operations
    """

    # Mouse Actions
    @abstractmethod
    async def mouse_down(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Perform a mouse down at the current or specified position."""
        pass

    @abstractmethod
    async def mouse_up(
        self, x: Optional[int] = None, y: Optional[int] = None, button: str = "left"
    ) -> Dict[str, Any]:
        """Perform a mouse up at the current or specified position."""
        pass

    @abstractmethod
    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a left click at the current or specified position."""
        pass

    @abstractmethod
    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a right click at the current or specified position."""
        pass

    @abstractmethod
    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        """Perform a double click at the current or specified position."""
        pass

    @abstractmethod
    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        """Move the cursor to the specified position."""
        pass

    @abstractmethod
    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag the cursor from current position to specified coordinates.

        Args:
            x: The x coordinate to drag to
            y: The y coordinate to drag to
            button: The mouse button to use ('left', 'middle', 'right')
            duration: How long the drag should take in seconds
        """
        pass

    @abstractmethod
    async def drag(
        self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        """Drag the cursor from current position to specified coordinates.

        Args:
            path: A list of tuples of x and y coordinates to drag to
            button: The mouse button to use ('left', 'middle', 'right')
            duration: How long the drag should take in seconds
        """
        pass

    # Keyboard Actions
    @abstractmethod
    async def key_down(self, key: str) -> Dict[str, Any]:
        """Press and hold the specified key."""
        pass

    @abstractmethod
    async def key_up(self, key: str) -> Dict[str, Any]:
        """Release the specified key."""
        pass

    @abstractmethod
    async def type_text(self, text: str) -> Dict[str, Any]:
        """Type the specified text."""
        pass

    @abstractmethod
    async def press_key(self, key: str) -> Dict[str, Any]:
        """Press the specified key."""
        pass

    @abstractmethod
    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        """Press a combination of keys together."""
        pass

    # Scrolling Actions
    @abstractmethod
    async def scroll(self, x: int, y: int) -> Dict[str, Any]:
        """Scroll the specified amount."""
        pass

    @abstractmethod
    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll down by the specified number of clicks."""
        pass

    @abstractmethod
    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        """Scroll up by the specified number of clicks."""
        pass

    # Screen Actions
    @abstractmethod
    async def screenshot(self) -> Dict[str, Any]:
        """Take a screenshot and return base64 encoded image data."""
        pass

    @abstractmethod
    async def get_screen_size(self) -> Dict[str, Any]:
        """Get the screen size of the VM."""
        pass

    @abstractmethod
    async def get_cursor_position(self) -> Dict[str, Any]:
        """Get the current cursor position."""
        pass

    # Clipboard Actions
    @abstractmethod
    async def copy_to_clipboard(self) -> Dict[str, Any]:
        """Get the current clipboard content."""
        pass

    @abstractmethod
    async def set_clipboard(self, text: str) -> Dict[str, Any]:
        """Set the clipboard content."""
        pass

    @abstractmethod
    async def run_command(self, command: str) -> Dict[str, Any]:
        """Run a command and return the output."""
        pass

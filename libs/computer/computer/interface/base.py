"""Base interface for computer control."""

from abc import ABC, abstractmethod
from typing import Optional, Dict, Any, Tuple, List
from ..logger import Logger, LogLevel


class BaseComputerInterface(ABC):
    """Base class for computer control interfaces."""

    def __init__(self, ip_address: str, username: str = "lume", password: str = "lume"):
        """Initialize interface.

        Args:
            ip_address: IP address of the computer to control
            username: Username for authentication
            password: Password for authentication
        """
        self.ip_address = ip_address
        self.username = username
        self.password = password
        self.logger = Logger("cua.interface", LogLevel.NORMAL)

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
    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Perform a left click."""
        pass

    @abstractmethod
    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Perform a right click."""
        pass

    @abstractmethod
    async def double_click(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Perform a double click."""
        pass

    @abstractmethod
    async def move_cursor(self, x: int, y: int) -> None:
        """Move the cursor to specified position."""
        pass

    @abstractmethod
    async def drag_to(self, x: int, y: int, button: str = "left", duration: float = 0.5) -> None:
        """Drag from current position to specified coordinates.

        Args:
            x: The x coordinate to drag to
            y: The y coordinate to drag to
            button: The mouse button to use ('left', 'middle', 'right')
            duration: How long the drag should take in seconds
        """
        pass

    @abstractmethod
    async def drag(self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5) -> None:
        """Drag the cursor along a path of coordinates.

        Args:
            path: List of (x, y) coordinate tuples defining the drag path
            button: The mouse button to use ('left', 'middle', 'right')
            duration: Total time in seconds that the drag operation should take
        """
        pass

    # Keyboard Actions
    @abstractmethod
    async def type_text(self, text: str) -> None:
        """Type the specified text."""
        pass

    @abstractmethod
    async def press_key(self, key: str) -> None:
        """Press a single key."""
        pass

    @abstractmethod
    async def hotkey(self, *keys: str) -> None:
        """Press multiple keys simultaneously."""
        pass

    # Scrolling Actions
    @abstractmethod
    async def scroll_down(self, clicks: int = 1) -> None:
        """Scroll down."""
        pass

    @abstractmethod
    async def scroll_up(self, clicks: int = 1) -> None:
        """Scroll up."""
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
        """Get current cursor position."""
        pass

    # Clipboard Actions
    @abstractmethod
    async def copy_to_clipboard(self) -> str:
        """Get clipboard content."""
        pass

    @abstractmethod
    async def set_clipboard(self, text: str) -> None:
        """Set clipboard content."""
        pass

    # File System Actions
    @abstractmethod
    async def file_exists(self, path: str) -> bool:
        """Check if file exists."""
        pass

    @abstractmethod
    async def directory_exists(self, path: str) -> bool:
        """Check if directory exists."""
        pass

    @abstractmethod
    async def run_command(self, command: str) -> Tuple[str, str]:
        """Run shell command."""
        pass

    # Accessibility Actions
    @abstractmethod
    async def get_accessibility_tree(self) -> Dict:
        """Get the accessibility tree of the current screen."""
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

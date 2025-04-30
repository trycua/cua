from abc import ABC, abstractmethod
from typing import Optional, Dict, Any, List, Tuple

class BaseAccessibilityHandler(ABC):
    """Abstract base class for OS-specific accessibility handlers."""
    
    @abstractmethod
    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree of the current window."""
        pass

    @abstractmethod
    async def find_element(self, role: Optional[str] = None,
                          title: Optional[str] = None,
                          value: Optional[str] = None) -> Dict[str, Any]:
        """Find an element in the accessibility tree by criteria."""
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
    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a left click at the current or specified position."""
        pass

    @abstractmethod
    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a right click at the current or specified position."""
        pass

    @abstractmethod
    async def double_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        """Perform a double click at the current or specified position."""
        pass

    @abstractmethod
    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        """Move the cursor to the specified position."""
        pass

    @abstractmethod
    async def drag_to(self, x: int, y: int, button: str = "left", duration: float = 0.5) -> Dict[str, Any]:
        """Drag the cursor from current position to specified coordinates.
        
        Args:
            x: The x coordinate to drag to
            y: The y coordinate to drag to
            button: The mouse button to use ('left', 'middle', 'right')
            duration: How long the drag should take in seconds
        """
        pass
    
    @abstractmethod
    async def drag(self, path: List[Tuple[int, int]], button: str = "left", duration: float = 0.5) -> Dict[str, Any]:
        """Drag the cursor from current position to specified coordinates.
        
        Args:
            path: A list of tuples of x and y coordinates to drag to
            button: The mouse button to use ('left', 'middle', 'right')
            duration: How long the drag should take in seconds
        """
        pass

    # Keyboard Actions
    @abstractmethod
    async def type_text(self, text: str) -> Dict[str, Any]:
        """Type the specified text."""
        pass

    @abstractmethod
    async def press_key(self, key: str) -> Dict[str, Any]:
        """Press the specified key."""
        pass

    @abstractmethod
    async def hotkey(self, *keys: str) -> Dict[str, Any]:
        """Press a combination of keys together."""
        pass

    # Scrolling Actions
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
"""
Base computer interface protocol for agent interactions.
"""

from typing import Protocol, Literal, List, Dict, Any, Union, Optional, runtime_checkable


@runtime_checkable
class AsyncComputerHandler(Protocol):
    """Protocol defining the interface for computer interactions."""
    
    # ==== Computer-Use-Preview Action Space ==== 

    async def get_environment(self) -> Literal["windows", "mac", "linux", "browser"]:
        """Get the current environment type."""
        ...
    
    async def get_dimensions(self) -> tuple[int, int]:
        """Get screen dimensions as (width, height)."""
        ...
    
    async def screenshot(self) -> str:
        """Take a screenshot and return as base64 string."""
        ...
    
    async def click(self, x: int, y: int, button: str = "left") -> None:
        """Click at coordinates with specified button."""
        ...
    
    async def double_click(self, x: int, y: int) -> None:
        """Double click at coordinates."""
        ...
    
    async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
        """Scroll at coordinates with specified scroll amounts."""
        ...
    
    async def type(self, text: str) -> None:
        """Type text."""
        ...
    
    async def wait(self, ms: int = 1000) -> None:
        """Wait for specified milliseconds."""
        ...
    
    async def move(self, x: int, y: int) -> None:
        """Move cursor to coordinates."""
        ...
    
    async def keypress(self, keys: Union[List[str], str]) -> None:
        """Press key combination."""
        ...
    
    async def drag(self, path: List[Dict[str, int]]) -> None:
        """Drag along specified path."""
        ...
    
    async def get_current_url(self) -> str:
        """Get current URL (for browser environments)."""
        ...
    
    # ==== Anthropic Action Space ==== 

    async def left_mouse_down(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse down at coordinates."""
        ...
    
    async def left_mouse_up(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
        """Left mouse up at coordinates."""
        ...

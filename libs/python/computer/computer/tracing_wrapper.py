"""
Tracing wrapper for computer interface that records API calls.
"""

from typing import Any, Dict, List, Optional, Tuple

from .interface.base import BaseComputerInterface


class TracingInterfaceWrapper:
    """
    Wrapper class that intercepts computer interface calls and records them for tracing.
    """

    def __init__(self, original_interface: BaseComputerInterface, tracing_instance):
        """
        Initialize the tracing wrapper.

        Args:
            original_interface: The original computer interface
            tracing_instance: The ComputerTracing instance
        """
        self._original_interface = original_interface
        self._tracing = tracing_instance

    def __getattr__(self, name):
        """
        Delegate attribute access to the original interface if not found in wrapper.
        """
        return getattr(self._original_interface, name)

    async def _record_call(
        self,
        method_name: str,
        args: Dict[str, Any],
        result: Any = None,
        error: Optional[Exception] = None,
    ):
        """
        Record an API call for tracing.

        Args:
            method_name: Name of the method called
            args: Arguments passed to the method
            result: Result returned by the method
            error: Exception raised, if any
        """
        if self._tracing.is_tracing:
            await self._tracing.record_api_call(method_name, args, result, error)

    # Mouse Actions
    async def left_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        """Perform a left mouse button click."""
        args = {"x": x, "y": y, "delay": delay}
        error = None
        try:
            result = await self._original_interface.left_click(x, y, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("left_click", args, None, error)

    async def right_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        """Perform a right mouse button click."""
        args = {"x": x, "y": y, "delay": delay}
        error = None
        try:
            result = await self._original_interface.right_click(x, y, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("right_click", args, None, error)

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None, delay: Optional[float] = None
    ) -> None:
        """Perform a double left mouse button click."""
        args = {"x": x, "y": y, "delay": delay}
        error = None
        try:
            result = await self._original_interface.double_click(x, y, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("double_click", args, None, error)

    async def move_cursor(self, x: int, y: int, delay: Optional[float] = None) -> None:
        """Move the cursor to the specified screen coordinates."""
        args = {"x": x, "y": y, "delay": delay}
        error = None
        try:
            result = await self._original_interface.move_cursor(x, y, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("move_cursor", args, None, error)

    async def drag_to(
        self,
        x: int,
        y: int,
        button: str = "left",
        duration: float = 0.5,
        delay: Optional[float] = None,
    ) -> None:
        """Drag from current position to specified coordinates."""
        args = {"x": x, "y": y, "button": button, "duration": duration, "delay": delay}
        error = None
        try:
            result = await self._original_interface.drag_to(x, y, button, duration, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("drag_to", args, None, error)

    async def drag(
        self,
        path: List[Tuple[int, int]],
        button: str = "left",
        duration: float = 0.5,
        delay: Optional[float] = None,
    ) -> None:
        """Drag the cursor along a path of coordinates."""
        args = {"path": path, "button": button, "duration": duration, "delay": delay}
        error = None
        try:
            result = await self._original_interface.drag(path, button, duration, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("drag", args, None, error)

    # Keyboard Actions
    async def key_down(self, key: str, delay: Optional[float] = None) -> None:
        """Press and hold a key."""
        args = {"key": key, "delay": delay}
        error = None
        try:
            result = await self._original_interface.key_down(key, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("key_down", args, None, error)

    async def key_up(self, key: str, delay: Optional[float] = None) -> None:
        """Release a previously pressed key."""
        args = {"key": key, "delay": delay}
        error = None
        try:
            result = await self._original_interface.key_up(key, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("key_up", args, None, error)

    async def type_text(self, text: str, delay: Optional[float] = None) -> None:
        """Type the specified text string."""
        args = {"text": text, "delay": delay}
        error = None
        try:
            result = await self._original_interface.type_text(text, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("type_text", args, None, error)

    async def press_key(self, key: str, delay: Optional[float] = None) -> None:
        """Press and release a single key."""
        args = {"key": key, "delay": delay}
        error = None
        try:
            result = await self._original_interface.press_key(key, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("press_key", args, None, error)

    async def hotkey(self, *keys: str, delay: Optional[float] = None) -> None:
        """Press multiple keys simultaneously (keyboard shortcut)."""
        args = {"keys": keys, "delay": delay}
        error = None
        try:
            result = await self._original_interface.hotkey(*keys, delay=delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("hotkey", args, None, error)

    # Scrolling Actions
    async def scroll(self, x: int, y: int, delay: Optional[float] = None) -> None:
        """Scroll the mouse wheel by specified amounts."""
        args = {"x": x, "y": y, "delay": delay}
        error = None
        try:
            result = await self._original_interface.scroll(x, y, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("scroll", args, None, error)

    async def scroll_down(self, clicks: int = 1, delay: Optional[float] = None) -> None:
        """Scroll down by the specified number of clicks."""
        args = {"clicks": clicks, "delay": delay}
        error = None
        try:
            result = await self._original_interface.scroll_down(clicks, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("scroll_down", args, None, error)

    async def scroll_up(self, clicks: int = 1, delay: Optional[float] = None) -> None:
        """Scroll up by the specified number of clicks."""
        args = {"clicks": clicks, "delay": delay}
        error = None
        try:
            result = await self._original_interface.scroll_up(clicks, delay)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("scroll_up", args, None, error)

    # Screen Actions
    async def screenshot(self) -> bytes:
        """Take a screenshot."""
        args = {}
        error = None
        result = None
        try:
            result = await self._original_interface.screenshot()
            return result
        except Exception as e:
            error = e
            raise
        finally:
            # For screenshots, we don't want to include the raw bytes in the trace args
            await self._record_call(
                "screenshot", args, "screenshot_taken" if result else None, error
            )

    async def get_screen_size(self) -> Dict[str, int]:
        """Get the screen dimensions."""
        args = {}
        error = None
        result = None
        try:
            result = await self._original_interface.get_screen_size()
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("get_screen_size", args, result, error)

    async def get_cursor_position(self) -> Dict[str, int]:
        """Get the current cursor position on screen."""
        args = {}
        error = None
        result = None
        try:
            result = await self._original_interface.get_cursor_position()
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("get_cursor_position", args, result, error)

    # Clipboard Actions
    async def copy_to_clipboard(self) -> str:
        """Get the current clipboard content."""
        args = {}
        error = None
        result = None
        try:
            result = await self._original_interface.copy_to_clipboard()
            return result
        except Exception as e:
            error = e
            raise
        finally:
            # Don't include clipboard content in trace for privacy
            await self._record_call(
                "copy_to_clipboard",
                args,
                f"content_length_{len(result)}" if result else None,
                error,
            )

    async def set_clipboard(self, text: str) -> None:
        """Set the clipboard content to the specified text."""
        # Don't include clipboard content in trace for privacy
        args = {"text_length": len(text)}
        error = None
        try:
            result = await self._original_interface.set_clipboard(text)
            return result
        except Exception as e:
            error = e
            raise
        finally:
            await self._record_call("set_clipboard", args, None, error)

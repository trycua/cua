import base64
from io import BytesIO
from typing import Any, Dict, List, Optional

from .base import BaseAutomationHandler

import pyautogui


class PyAutoGUIAutomationHandler(BaseAutomationHandler):
    # Mouse Actions
    async def left_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.click()
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def right_click(self, x: Optional[int] = None, y: Optional[int] = None) -> Dict[str, Any]:
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.rightClick()
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def double_click(
        self, x: Optional[int] = None, y: Optional[int] = None
    ) -> Dict[str, Any]:
        try:
            if x is not None and y is not None:
                pyautogui.moveTo(x, y)
            pyautogui.doubleClick(interval=0.1)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def move_cursor(self, x: int, y: int) -> Dict[str, Any]:
        try:
            pyautogui.moveTo(x, y)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def drag_to(
        self, x: int, y: int, button: str = "left", duration: float = 0.5
    ) -> Dict[str, Any]:
        try:
            pyautogui.dragTo(x, y, button=button, duration=duration)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Keyboard Actions
    async def type_text(self, text: str) -> Dict[str, Any]:
        try:
            pyautogui.write(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def press_key(self, key: str) -> Dict[str, Any]:
        try:
            pyautogui.press(key)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def hotkey(self, keys: List[str]) -> Dict[str, Any]:
        try:
            pyautogui.hotkey(*keys)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Scrolling Actions
    async def scroll_down(self, clicks: int = 1) -> Dict[str, Any]:
        try:
            pyautogui.scroll(-clicks)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def scroll_up(self, clicks: int = 1) -> Dict[str, Any]:
        try:
            pyautogui.scroll(clicks)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Screen Actions
    async def screenshot(self) -> Dict[str, Any]:
        try:
            from PIL import Image

            screenshot = pyautogui.screenshot()
            if not isinstance(screenshot, Image.Image):
                return {"success": False, "error": "Failed to capture screenshot"}

            buffered = BytesIO()
            screenshot.save(buffered, format="PNG", optimize=True)
            buffered.seek(0)
            image_data = base64.b64encode(buffered.getvalue()).decode()
            return {"success": True, "image_data": image_data}
        except Exception as e:
            return {"success": False, "error": f"Screenshot error: {str(e)}"}

    async def get_screen_size(self) -> Dict[str, Any]:
        try:
            size = pyautogui.size()
            return {"success": True, "size": {"width": size.width, "height": size.height}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def get_cursor_position(self) -> Dict[str, Any]:
        try:
            pos = pyautogui.position()
            return {"success": True, "position": {"x": pos.x, "y": pos.y}}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # Clipboard Actions
    async def copy_to_clipboard(self) -> Dict[str, Any]:
        try:
            import pyperclip

            content = pyperclip.paste()
            return {"success": True, "content": content}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def set_clipboard(self, text: str) -> Dict[str, Any]:
        try:
            import pyperclip

            pyperclip.copy(text)
            return {"success": True}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def run_command(self, command: str) -> Dict[str, Any]:
        """Run a shell command and return its output."""
        try:
            import subprocess

            process = subprocess.run(command, shell=True, capture_output=True, text=True)
            return {"success": True, "stdout": process.stdout, "stderr": process.stderr}
        except Exception as e:
            return {"success": False, "error": str(e)}

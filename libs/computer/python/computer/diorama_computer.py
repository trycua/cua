import asyncio
from .interface.models import KeyType, Key

class DioramaComputer:
    """
    A Computer-compatible proxy for Diorama that sends commands over the ComputerInterface.
    """
    def __init__(self, computer, apps):
        self.computer = computer
        self.apps = apps
        self.interface = DioramaComputerInterface(computer, apps)
        self._initialized = False

    async def __aenter__(self):
        self._initialized = True
        return self

    async def run(self):
        if not self._initialized:
            await self.__aenter__()
        return self

class DioramaComputerInterface:
    """
    Diorama Interface proxy that sends diorama_cmds via the Computer's interface.
    """
    def __init__(self, computer, apps):
        self.computer = computer
        self.apps = apps
        self._scene_size = None

    async def _send_cmd(self, action, arguments=None):
        arguments = arguments or {}
        arguments = {"app_list": self.apps, **arguments}
        # Use the computer's interface (must be initialized)
        iface = getattr(self.computer, "_interface", None)
        if iface is None:
            raise RuntimeError("Computer interface not initialized. Call run() first.")
        result = await iface.diorama_cmd(action, arguments)
        if not result.get("success"):
            raise RuntimeError(f"Diorama command failed: {result.get('error')}\n{result.get('trace')}")
        return result.get("result")

    async def screenshot(self, as_bytes=True):
        from PIL import Image
        import base64
        result = await self._send_cmd("screenshot")
        # assume result is a b64 string of an image
        img_bytes = base64.b64decode(result)
        import io
        img = Image.open(io.BytesIO(img_bytes))
        self._scene_size = img.size
        return img_bytes if as_bytes else img

    async def get_screen_size(self):
        if not self._scene_size:
            await self.screenshot(as_bytes=False)
        return {"width": self._scene_size[0], "height": self._scene_size[1]}

    async def move_cursor(self, x, y):
        await self._send_cmd("move_cursor", {"x": x, "y": y})

    async def left_click(self, x=None, y=None):
        await self._send_cmd("left_click", {"x": x, "y": y})

    async def right_click(self, x=None, y=None):
        await self._send_cmd("right_click", {"x": x, "y": y})

    async def double_click(self, x=None, y=None):
        await self._send_cmd("double_click", {"x": x, "y": y})

    async def scroll_up(self, clicks=1):
        await self._send_cmd("scroll_up", {"clicks": clicks})

    async def scroll_down(self, clicks=1):
        await self._send_cmd("scroll_down", {"clicks": clicks})

    async def drag_to(self, x, y, duration=0.5):
        await self._send_cmd("drag_to", {"x": x, "y": y, "duration": duration})

    async def get_cursor_position(self):
        return await self._send_cmd("get_cursor_position")

    async def type_text(self, text):
        await self._send_cmd("type_text", {"text": text})

    async def press_key(self, key):
        await self._send_cmd("press_key", {"key": key})

    async def hotkey(self, *keys):
        actual_keys = []
        for key in keys:
            if isinstance(key, Key):
                actual_keys.append(key.value)
            elif isinstance(key, str):
                # Try to convert to enum if it matches a known key
                key_or_enum = Key.from_string(key)
                actual_keys.append(key_or_enum.value if isinstance(key_or_enum, Key) else key_or_enum)
            else:
                raise ValueError(f"Invalid key type: {type(key)}. Must be Key enum or string.")
        await self._send_cmd("hotkey", {"keys": actual_keys})

    async def to_screen_coordinates(self, x, y):
        return await self._send_cmd("to_screen_coordinates", {"x": x, "y": y})

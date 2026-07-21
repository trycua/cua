from __future__ import annotations

import json
import os
import platform
import socket
import tempfile
import threading
import unittest
from pathlib import Path


def _library_name() -> str:
    if os.name == "nt":
        return "cua_driver_sdk.dll"
    if platform.system() == "Darwin":
        return "libcua_driver_sdk.dylib"
    return "libcua_driver_sdk.so"


LIBRARY = Path(__file__).parents[1] / "src" / "cua_driver" / _library_name()


@unittest.skipUnless(LIBRARY.exists(), "host-native UniFFI library is not staged")
@unittest.skipIf(os.name == "nt", "Unix socket fixture")
class UniFfiLoaderTests(unittest.TestCase):
    def test_generated_python_calls_the_rust_daemon_interface(self) -> None:
        from cua_driver.native import (
            CuaDriver,
            EffectiveScope,
            GetDesktopStateInput,
            StartSessionOutput,
        )

        self.assertIsNotNone(EffectiveScope)
        self.assertIsNotNone(StartSessionOutput)

        with tempfile.TemporaryDirectory() as directory:
            socket_path = str(Path(directory) / "driver.sock")
            listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            listener.bind(socket_path)
            listener.listen(1)
            captured: list[dict[str, object]] = []

            def serve() -> None:
                connection, _ = listener.accept()
                with connection:
                    line = connection.makefile("r", encoding="utf-8").readline()
                    captured.append(json.loads(line))
                    response = {
                        "ok": True,
                        "result": {
                            "content": [
                                {"type": "text", "text": "python ffi"},
                                {
                                    "type": "image",
                                    "mimeType": "image/png",
                                    "data": "cG5n",
                                },
                            ],
                            "structuredContent": {"verified": True},
                            "isError": False,
                        },
                    }
                    connection.sendall((json.dumps(response) + "\n").encode())

            server = threading.Thread(target=serve)
            server.start()
            driver = CuaDriver.connect(socket_path)
            expected_methods = {
                "start_session",
                "escalate_session",
                "get_session_state",
                "end_session",
                "get_desktop_state",
                "get_screen_size",
                "get_cursor_position",
                "move_cursor",
                "click",
                "drag",
                "scroll",
                "type_text",
                "press_key",
                "hotkey",
            }
            self.assertTrue(all(hasattr(driver, name) for name in expected_methods))
            result = driver.get_desktop_state(
                GetDesktopStateInput(session="python-run", screenshot_out_file=None)
            )
            server.join(timeout=5)
            listener.close()

        self.assertEqual(result.text, "python ffi")
        self.assertEqual(result.images[0].mime_type, "image/png")
        self.assertTrue(result.verified)
        self.assertEqual(captured[0]["name"], "get_desktop_state")
        self.assertEqual(captured[0]["args"], {"session": "python-run"})


if os.environ.get("CUA_DRIVER_REQUIRE_UNIFFI") == "1" and not LIBRARY.exists():
    raise RuntimeError(f"required staged UniFFI library is missing: {LIBRARY}")


if __name__ == "__main__":
    unittest.main()

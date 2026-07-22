from __future__ import annotations

import asyncio
import importlib.util
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
class SdkLoaderTests(unittest.TestCase):
    def test_generated_python_embedded_host_owns_the_rust_lifecycle(self) -> None:
        from cua_driver import CuaDriver, EmbeddedCuaDriverHost

        with tempfile.TemporaryDirectory() as directory:
            binary_path = Path(directory) / "fake cua-driver"
            binary_path.write_text(
                """#!/usr/bin/env python3
import json
import os
import select
import socket
import sys

args = sys.argv[1:]
socket_path = args[args.index("--socket") + 1]
host_bundle_id = args[args.index("--host-bundle-id") + 1]
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(socket_path)
os.chmod(socket_path, 0o600)
server.listen(8)
while True:
    readable, _, _ = select.select([server, sys.stdin.buffer], [], [], 1)
    if sys.stdin.buffer in readable:
        if not sys.stdin.buffer.read(1):
            break
    if server in readable:
        connection, _ = server.accept()
        with connection:
            request = json.loads(connection.makefile("r", encoding="utf-8").readline())
            if request["method"] == "metadata":
                result = {
                    "driver_version": "0.10.0",
                    "contract_version": "0.2.0",
                    "tools_list_schema_version": "1",
                    "capability_version": "1",
                    "mcp_protocol_version": "2025-06-18",
                    "pid": os.getpid(),
                    "embedded": True,
                    "host_bundle_id": host_bundle_id,
                }
            else:
                result = {"tools": [{"name": "embedded_fixture"}]}
            connection.sendall((json.dumps({"ok": True, "result": result}) + "\\n").encode())
server.close()
try:
    os.unlink(socket_path)
except FileNotFoundError:
    pass
""",
                encoding="utf-8",
            )
            binary_path.chmod(0o755)

            async def scenario() -> str:
                host = EmbeddedCuaDriverHost(
                    str(binary_path), "com.example.python-embedded"
                )
                connection = await host.start()
                driver = CuaDriver.connect(connection.socket_path)
                metadata = driver.metadata()
                self.assertTrue(metadata.embedded)
                self.assertEqual(metadata.pid, connection.pid)
                self.assertEqual(
                    metadata.host_bundle_id, "com.example.python-embedded"
                )
                self.assertEqual(
                    json.loads(driver.list_tools_json()),
                    {"tools": [{"name": "embedded_fixture"}]},
                )
                await host.stop()
                return connection.socket_path

            socket_path = asyncio.run(scenario())
            self.assertFalse(Path(socket_path).exists())

    def test_generated_python_sdk_calls_the_rust_daemon_interface(self) -> None:
        import cua_driver
        from cua_driver import (
            CuaDriver,
            EffectiveScope,
            GetDesktopStateInput,
            StartSessionOutput,
        )

        self.assertIsNotNone(EffectiveScope)
        self.assertIsNotNone(StartSessionOutput)
        self.assertIs(cua_driver.CuaDriver, CuaDriver)
        self.assertFalse(hasattr(cua_driver, "AsyncCuaDriver"))
        self.assertFalse(hasattr(cua_driver, "StdioMcpTransport"))
        self.assertIsNone(importlib.util.find_spec("cua_driver.sdk"))
        self.assertIsNone(importlib.util.find_spec("cua_driver.native"))

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
        self.assertEqual(captured[0]["client_kind"], "python_sdk")


if os.environ.get("CUA_DRIVER_REQUIRE_UNIFFI") == "1" and not LIBRARY.exists():
    raise RuntimeError(f"required staged UniFFI library is missing: {LIBRARY}")


if __name__ == "__main__":
    unittest.main()

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
                    "contract_version": "0.7.0",
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
                metadata = await driver.metadata()
                self.assertTrue(metadata.embedded)
                self.assertEqual(metadata.pid, connection.pid)
                self.assertEqual(
                    metadata.host_bundle_id, "com.example.python-embedded"
                )
                self.assertEqual(
                    json.loads(await driver.list_tools_json()),
                    {"tools": [{"name": "embedded_fixture"}]},
                )
                await host.stop()
                return connection.socket_path

            socket_path = asyncio.run(scenario())
            self.assertFalse(Path(socket_path).exists())

    def test_generated_python_sdk_calls_the_rust_daemon_interface(self) -> None:
        import cua_driver
        from cua_driver import (
            ActionEffect,
            ActionRoute,
            ClickButton,
            ClickInput,
            CuaDriver,
            DesktopScope,
            EffectiveScope,
            StatePredicate,
            StartSessionOutput,
            VerificationStatus,
            VerifyStateInput,
            WindowPredicate,
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
            listener.listen(4)
            captured: list[dict[str, object]] = []

            def serve() -> None:
                while len(captured) < 2:
                    connection, _ = listener.accept()
                    with connection:
                        line = connection.makefile("r", encoding="utf-8").readline()
                        request = json.loads(line)
                        if request["method"] == "metadata":
                            result = {
                                "driver_version": "0.12.6",
                                "contract_version": "0.7.0",
                                "tools_list_schema_version": "1",
                                "capability_version": "1",
                                "mcp_protocol_version": "2025-06-18",
                                "pid": os.getpid(),
                                "embedded": False,
                            }
                        else:
                            captured.append(request)
                            if request["name"] == "verify_state":
                                structured = {
                                    "status": "satisfied",
                                    "stable": True,
                                    "elapsed_ms": 12,
                                    "samples": 2,
                                    "predicates": [],
                                }
                            else:
                                structured = {
                                    "effect": "unverifiable",
                                    "route": "global_input",
                                    "delivery": {"mode": "not_applicable"},
                                }
                            result = {
                                "content": [
                                    {"type": "text", "text": "python ffi"},
                                    {
                                        "type": "image",
                                        "mimeType": "image/png",
                                        "data": "cG5n",
                                    },
                                ],
                                "structuredContent": structured,
                                "isError": False,
                            }
                        response = {"ok": True, "result": result}
                        connection.sendall((json.dumps(response) + "\n").encode())

            server = threading.Thread(target=serve)
            server.start()
            driver = CuaDriver.connect(socket_path)
            expected_methods = {
                "start_session",
                "escalate_session",
                "get_session",
                "list_sessions",
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
                "verify_state",
            }
            self.assertTrue(all(hasattr(driver, name) for name in expected_methods))
            verification_result = asyncio.run(
                driver.verify_state(
                    VerifyStateInput(
                        pid=123,
                        window_id=456,
                        expect=[
                            StatePredicate(
                                window=WindowPredicate(exists=True, bounds=None),
                                element=None,
                            )
                        ],
                        session="python-run",
                        timeout_ms=0,
                        stable_samples=1,
                        include_screenshot=True,
                    )
                )
            )
            action_result = asyncio.run(
                driver.click(
                    ClickInput(
                        x=12.0,
                        y=34.0,
                        target=None,
                        scope=DesktopScope.DESKTOP,
                        session="python-run",
                        button=ClickButton.LEFT,
                        count=1,
                    )
                )
            )
            server.join(timeout=5)
            listener.close()

        self.assertEqual(verification_result.text, "python ffi")
        self.assertEqual(verification_result.images[0].mime_type, "image/png")
        self.assertIsNone(verification_result.action)
        self.assertEqual(
            verification_result.verification.status, VerificationStatus.SATISFIED
        )
        self.assertIsNone(action_result.verification)
        self.assertEqual(action_result.action.effect, ActionEffect.UNVERIFIABLE)
        self.assertEqual(action_result.action.route, ActionRoute.GLOBAL_INPUT)
        self.assertFalse(hasattr(action_result, "verified"))
        self.assertEqual(captured[0]["name"], "verify_state")
        self.assertEqual(
            captured[0]["args"],
            {
                "pid": 123,
                "window_id": 456,
                "expect": [{"window": {"exists": True}}],
                "session": "python-run",
                "timeout_ms": 0,
                "stable_samples": 1,
                "include_screenshot": True,
            },
        )
        self.assertEqual(captured[0]["client_kind"], "python_sdk")
        self.assertEqual(captured[1]["name"], "click")
        self.assertEqual(
            captured[1]["args"],
            {
                "x": 12.0,
                "y": 34.0,
                "scope": "desktop",
                "session": "python-run",
                "button": "left",
                "count": 1,
            },
        )

    def test_generated_python_sdk_can_own_the_runtime_in_process(self) -> None:
        from cua_driver import CuaDriver, DriverExecutionMode

        async def scenario() -> None:
            driver = CuaDriver.create()
            self.assertEqual(driver.execution_mode(), DriverExecutionMode.EMBEDDED)
            self.assertEqual(driver.socket_path(), "")
            self.assertTrue(driver.is_available())
            metadata = await driver.metadata()
            self.assertTrue(metadata.embedded)
            self.assertEqual(metadata.pid, os.getpid())
            await driver.shutdown()
            self.assertFalse(driver.is_available())

        asyncio.run(scenario())


if os.environ.get("CUA_DRIVER_REQUIRE_UNIFFI") == "1" and not LIBRARY.exists():
    raise RuntimeError(f"required staged UniFFI library is missing: {LIBRARY}")


if __name__ == "__main__":
    unittest.main()

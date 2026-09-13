"""Exercise foreign callbacks through the generated native library."""

import asyncio
import json
import sys
import unittest
from pathlib import Path

from cua_driver import (
    CuaDriver,
    DriverError,
    DriverExecutionMode,
    ForeignDriverChannelCapabilities,
    ForeignDriverBoundChannel,
    ForeignDriverChannelError,
    ForeignDriverChannelIdentity,
    ForeignDriverEnvelopeChannel,
    ForeignDriverResponseEnvelope,
    GetScreenSizeInput,
    SessionPermissionMode,
    TrustedSessionOptions,
    connect_remote_channel,
    create_remote_trusted_session,
)


class Carrier(ForeignDriverEnvelopeChannel):
    def __init__(self):
        self.principal = "test-carrier-context"
        self.generation = "test-generation"
        self.requests = []
        self.closed = asyncio.Event()
        self.started = asyncio.Event()
        self.cancelled = asyncio.Event()
        self.cancelled_id = None
        self.bound = None
        self.response_mode = "ok"
        self.result = None
        self.supports_cancellation = True
        self.identity_error = False
        self.minimum_version = 1
        self.negotiate_wait = False

    def identity(self):
        if self.identity_error:
            raise ForeignDriverChannelError.Failed("fixture identity unavailable")
        return ForeignDriverChannelIdentity(
            authenticated_principal=self.principal, connection_generation=self.generation
        )

    async def negotiate(self):
        if self.negotiate_wait:
            self.started.set()
            await asyncio.Event().wait()
        return ForeignDriverChannelCapabilities(
            minimum_envelope_version=self.minimum_version,
            maximum_envelope_version=self.minimum_version,
            supports_cancellation=self.supports_cancellation,
        )

    async def exchange(self, request):
        self.requests.append(request)
        self.started.set()
        if self.response_mode == "wait":
            await asyncio.Event().wait()
        if self.response_mode == "error":
            raise ForeignDriverChannelError.Failed("fixture disconnected")
        result = self.result if self.result is not None else {
            "content": [{"type": "text", "text": "carrier response"}],
            "isError": False,
        }
        return ForeignDriverResponseEnvelope(
            envelope_version=request.envelope_version,
            request_id="wrong-id" if self.response_mode == "mismatch" else request.request_id,
            ok=self.response_mode != "denied",
            result_json="invalid-json" if self.response_mode == "invalid" else json.dumps(result),
            error="fixture denied" if self.response_mode == "denied" else None,
            error_code="policy_denied" if self.response_mode == "denied" else None,
            completion_known=self.response_mode != "unknown",
        )

    async def bind_session(self, options):
        self.options = options
        if self.bound is None:
            self.bound = Carrier()
        return ForeignDriverBoundChannel(channel=self.bound)

    async def cancel(self, request_id):
        self.cancelled_id = request_id
        self.cancelled.set()

    async def close(self):
        self.closed.set()


def session_options():
    return TrustedSessionOptions(
        public_session="test-session",
        mode=SessionPermissionMode.STANDARD,
        ttl_seconds=60,
        idle_ttl_seconds=30,
        capability_manifest_path=None,
        bounded_manifest_path=None,
    )


class RemoteChannelTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.unraisable_errors = []
        self.original_unraisablehook = sys.unraisablehook
        sys.unraisablehook = self.unraisable_errors.append
        self.assertTrue(asyncio.get_running_loop().get_debug())
        self.carrier = Carrier()
        self.driver = connect_remote_channel(self.carrier)

    async def asyncTearDown(self):
        await self.driver.shutdown()
        await asyncio.sleep(0)
        sys.unraisablehook = self.original_unraisablehook
        self.assertEqual(self.unraisable_errors, [], "native callback raised an unhandled exception")

    async def test_generated_foreign_future_drop_uses_threadsafe_scheduler(self):
        from cua_driver import _native

        source = Path(_native.__file__).read_text()
        self.assertIn("eventloop.call_soon_threadsafe(_uniffi_cancel_task, task)", source)
        self.assertNotIn("eventloop.call_soon(_uniffi_cancel_task, task)", source)

    async def test_canonical_driver_projection_and_bound_lifecycle(self):
        self.assertIsInstance(self.driver, CuaDriver)
        self.assertEqual(self.driver.execution_mode(), DriverExecutionMode.REMOTE)
        result = await self.driver.call_tool("health_report", '{"sample":null}')
        self.assertEqual(result.text, "carrier response")
        request = self.carrier.requests[0]
        self.assertEqual(request.operation, "call")
        self.assertEqual(request.name, "health_report")
        self.assertEqual(json.loads(request.arguments_json), {"sample": None})
        self.assertGreater(request.deadline_unix_ms, 0)
        self.assertEqual(
            (await self.driver.get_screen_size(GetScreenSizeInput(session=None))).text,
            "carrier response",
        )
        self.assertEqual(self.carrier.requests[-1].name, "get_screen_size")
        session = await create_remote_trusted_session(self.driver, session_options())
        self.assertEqual((await session.call_tool("health_report", "{}")).text, "carrier response")
        session.close()
        await asyncio.wait_for(self.carrier.bound.closed.wait(), 2)
        with self.assertRaises(DriverError.Shutdown):
            await session.call_tool("health_report", "{}")
        await self.driver.shutdown()
        self.assertTrue(self.carrier.closed.is_set())

    async def test_identity_is_snapshotted_and_bound_identity_must_match(self):
        self.carrier.principal = "changed-after-connect"
        session = await create_remote_trusted_session(self.driver, session_options())
        session.close()
        await asyncio.wait_for(self.carrier.bound.closed.wait(), 2)
        self.carrier.bound = Carrier()
        self.carrier.bound.generation = "different-generation"
        with self.assertRaises(DriverError.Remote):
            await create_remote_trusted_session(self.driver, session_options())
        self.assertTrue(self.carrier.bound.closed.is_set())
        self.carrier.bound = Carrier()
        self.carrier.bound.identity_error = True
        with self.assertRaises(DriverError.Remote):
            await create_remote_trusted_session(self.driver, session_options())
        self.assertTrue(self.carrier.bound.closed.is_set())

    async def test_negotiation_fails_before_dispatch(self):
        self.carrier.supports_cancellation = False
        with self.assertRaises(DriverError.Protocol):
            await self.driver.call_tool("health_report", "{}")
        self.assertEqual(self.carrier.requests, [])

    async def test_response_validation_and_unknown_completion(self):
        for mode, error in [
            ("mismatch", DriverError.Protocol),
            ("denied", DriverError.Tool),
            ("unknown", DriverError.ActionInterrupted),
            ("invalid", DriverError.ActionInterrupted),
            ("error", DriverError.ActionInterrupted),
        ]:
            with self.subTest(mode=mode):
                self.carrier.response_mode = mode
                with self.assertRaises(error):
                    await self.driver.call_tool("health_report", "{}")

    async def test_cancel_forwards_exact_request_identity(self):
        self.carrier.response_mode = "wait"
        task = asyncio.create_task(self.driver.call_tool("health_report", "{}"))
        await asyncio.wait_for(self.carrier.started.wait(), 2)
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        await asyncio.wait_for(self.carrier.cancelled.wait(), 2)
        self.assertEqual(self.carrier.cancelled_id, self.carrier.requests[0].request_id)

    async def test_rejected_bound_negotiation_closes_channel(self):
        for minimum_version, cancellation in [(1, False), (2, True)]:
            self.carrier.bound = Carrier()
            self.carrier.bound.minimum_version = minimum_version
            self.carrier.bound.supports_cancellation = cancellation
            with self.assertRaises(DriverError.Protocol):
                await create_remote_trusted_session(self.driver, session_options())
            self.assertTrue(self.carrier.bound.closed.is_set())
            self.assertEqual(self.carrier.bound.requests, [])

    async def test_cancelled_bound_negotiation_closes_channel(self):
        self.carrier.bound = Carrier()
        self.carrier.bound.negotiate_wait = True
        task = asyncio.create_task(create_remote_trusted_session(self.driver, session_options()))
        await asyncio.wait_for(self.carrier.bound.started.wait(), 2)
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        await asyncio.wait_for(self.carrier.bound.closed.wait(), 2)

    async def test_channels_on_independent_loops_and_wrong_loop_rejected(self):
        async def other_loop():
            with self.assertRaisesRegex(RuntimeError, "different asyncio event loop"):
                connect_remote_channel(self.carrier)
            carrier = Carrier()
            driver = connect_remote_channel(carrier)
            try:
                self.assertEqual((await driver.call_tool("health_report", "{}")).text, "carrier response")
                session = await create_remote_trusted_session(driver, session_options())
                session.close()
                await asyncio.wait_for(carrier.bound.closed.wait(), 2)
            finally:
                await driver.shutdown()

        await asyncio.gather(
            asyncio.to_thread(lambda: asyncio.run(other_loop(), debug=True)),
            self.driver.call_tool("health_report", "{}"),
        )

    async def test_closed_owner_loop_returns_error_without_unraisable_callback(self):
        async def create_on_other_loop():
            return connect_remote_channel(Carrier())

        driver = await asyncio.to_thread(lambda: asyncio.run(create_on_other_loop(), debug=True))
        with self.assertRaisesRegex(DriverError.Remote, "event loop is closed"):
            await driver.call_tool("health_report", "{}")
        with self.assertRaisesRegex(DriverError.Remote, "event loop is closed"):
            await driver.shutdown()


class NativeWindowTests(unittest.IsolatedAsyncioTestCase):
    async def test_typed_discovery_snapshot_and_token_click_cross_ffi(self):
        import cua_driver as sdk

        carrier = Carrier()
        driver = connect_remote_channel(carrier)

        def respond(structured, *, images=False, error=False):
            carrier.result = {
                "content": [{"type": "text", "text": "native window fixture"}],
                "structuredContent": structured,
                "isError": error,
            }
            if images:
                carrier.result["content"].append(
                    {"type": "image", "mimeType": "image/png", "data": "cG5n"}
                )

        try:
            respond({"apps": [{"pid": 42, "name": "Editor", "running": True, "active": False}]})
            apps = await driver.list_apps(sdk.ListAppsInput())
            self.assertIsInstance(apps, sdk.ListAppsOutput)
            self.assertEqual(apps.apps[0].name, "Editor")
            self.assertEqual(json.loads(carrier.requests[-1].arguments_json), {})

            respond(
                {
                    "windows": [
                        {
                            "pid": 42,
                            "window_id": 123,
                            "app_name": "Editor",
                            "title": "Document",
                            "bounds": {"x": 0, "y": 0, "width": 800, "height": 600},
                            "is_on_screen": True,
                            "z_index": None,
                        }
                    ]
                }
            )
            windows = await driver.list_windows(sdk.ListWindowsInput(pid=42, on_screen_only=True))
            self.assertIsInstance(windows.windows[0], sdk.WindowInfo)
            self.assertEqual(windows.windows[0].bounds.width, 800)
            self.assertIsNone(windows.windows[0].z_index)
            self.assertEqual(
                json.loads(carrier.requests[-1].arguments_json), {"pid": 42, "on_screen_only": True}
            )

            respond(
                {
                    "pid": 42,
                    "window_id": 123,
                    "snapshot_id": "snapshot-1",
                    "screenshot_width": 800,
                    "screenshot_height": 600,
                    "elements": [
                        {
                            "element_index": 0,
                            "role": "button",
                            "depth": 0,
                            "element_token": "fresh-token",
                            "label": "Save",
                        }
                    ],
                },
                images=True,
            )
            state = await driver.get_window_state(
                sdk.GetWindowStateInput(
                    pid=42,
                    window_id=123,
                    session="native-window",
                    query="Save",
                    include_screenshot=True,
                    include_accessibility_tree=True,
                    screenshot_out_file=None,
                    max_elements=10,
                    max_depth=3,
                    max_dimension=800,
                )
            )
            self.assertIsInstance(state, sdk.WindowStateOutput)
            self.assertIsInstance(state.elements[0], sdk.WindowElement)
            self.assertEqual(
                state.images[0], sdk.SnapshotImage(mime_type="image/png", data_base64="cG5n")
            )
            self.assertEqual(
                json.loads(carrier.requests[-1].arguments_json),
                {
                    "pid": 42,
                    "window_id": 123,
                    "session": "native-window",
                    "query": "Save",
                    "include_screenshot": True,
                    "include_accessibility_tree": True,
                    "max_elements": 10,
                    "max_depth": 3,
                    "max_dimension": 800,
                },
            )

            respond(
                {
                    "effect": "unverifiable",
                    "route": "global_input",
                    "delivery": {"mode": "not_applicable"},
                }
            )
            click = sdk.ClickInput(
                target=sdk.ActionTarget.WINDOW(pid=42, window_id=123),
                position=sdk.ClickPosition.ELEMENT(element_token=state.elements[0].element_token),
                delivery_mode=sdk.InputDeliveryMode.BACKGROUND,
                session="native-window",
                button=None,
                count=None,
            )
            action = await driver.click(click)
            self.assertIsInstance(action, sdk.ActionResult)
            self.assertEqual(action.effect, sdk.ActionEffect.UNVERIFIABLE)
            self.assertEqual(
                json.loads(carrier.requests[-1].arguments_json),
                {
                    "target": {"kind": "window", "pid": 42, "window_id": 123},
                    "element_token": "fresh-token",
                    "delivery_mode": "background",
                    "session": "native-window",
                },
            )

            # These service refusals test SDK error propagation, not native token validation.
            for token, window_id, code in [
                ("stale-token", 123, "stale_element_token"),
                ("fresh-token", 124, "element_target_mismatch"),
            ]:
                respond({"code": code}, error=True)
                click.position = sdk.ClickPosition.ELEMENT(element_token=token)
                click.target = sdk.ActionTarget.WINDOW(pid=42, window_id=window_id)
                with self.assertRaises(sdk.DriverError.Tool) as error:
                    await driver.click(click)
                self.assertEqual(error.exception.tool, "click")
                self.assertEqual(error.exception.error_code, code)
                request = json.loads(carrier.requests[-1].arguments_json)
                self.assertEqual(request["element_token"], token)
                self.assertEqual(request["target"]["window_id"], window_id)
        finally:
            await driver.shutdown()


if __name__ == "__main__":
    unittest.main()

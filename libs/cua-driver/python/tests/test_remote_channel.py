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
from cua_driver._native import uniffi_set_event_loop


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
        self.supports_cancellation = True
        self.identity_error = False

    def identity(self):
        if self.identity_error:
            raise ForeignDriverChannelError.Failed("fixture identity unavailable")
        return ForeignDriverChannelIdentity(
            authenticated_principal=self.principal, connection_generation=self.generation
        )

    async def negotiate(self):
        return ForeignDriverChannelCapabilities(
            minimum_envelope_version=1,
            maximum_envelope_version=1,
            supports_cancellation=self.supports_cancellation,
        )

    async def exchange(self, request):
        self.requests.append(request)
        self.started.set()
        if self.response_mode == "wait":
            await asyncio.Event().wait()
        if self.response_mode == "error":
            raise ForeignDriverChannelError.Failed("fixture disconnected")
        result = {
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
        uniffi_set_event_loop(asyncio.get_running_loop())
        self.carrier = Carrier()
        self.driver = connect_remote_channel(self.carrier)

    async def asyncTearDown(self):
        await self.driver.shutdown()
        await asyncio.sleep(0)
        uniffi_set_event_loop(None)
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


if __name__ == "__main__":
    unittest.main()

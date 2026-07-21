import asyncio
import unittest
from contract_fixture import (ScriptedHttpClient, client, expected_lifecycle,
    expected_service_calls, offline_sandbox, pool_spec, run_lifecycle,
    service_request)
from cyclops_sdk import CreatePoolRequest, HttpClient, HttpError, SdkError

class AsyncClientContractTest(unittest.IsolatedAsyncioTestCase):
    async def test_full_typed_lifecycle_has_exact_native_callback_contract(self):
        pool, claim, sandbox, service = await run_lifecycle(ScriptedHttpClient(expected_lifecycle()))
        self.assertEqual((pool.metadata.name, claim.metadata.name, sandbox.name, service.status), ('default','default','offline-sandbox',202))
    async def test_callback_http_error_propagates_through_native_async_call(self):
        class FailingHttpClient(HttpClient):
            async def execute(self, request):
                raise HttpError.Transport('offline transport')
        with self.assertRaises(SdkError.Transport) as raised:
            await client(FailingHttpClient()).create_pool(CreatePoolRequest(namespace='default', spec=pool_spec()))
        self.assertEqual(raised.exception.reason, 'offline transport')

    async def test_native_callback_distinguishes_absent_empty_and_binary_bodies(self):
        transport = ScriptedHttpClient(expected_service_calls(
            (None, b''),
            (b'', b'\x00\xff'),
            (b'\x00\xff', b'\xff\x00'),
        ))
        sdk = client(transport)
        responses = [
            await sdk.service_request(offline_sandbox(), 'mcp', '/mcp', service_request(None)),
            await sdk.service_request(offline_sandbox(), 'mcp', '/mcp', service_request(b'')),
            await sdk.service_request(offline_sandbox(), 'mcp', '/mcp', service_request(b'\x00\xff')),
        ]
        self.assertIsNone(transport.requests[1].body)
        self.assertEqual(transport.requests[2].body, b'')
        self.assertEqual(transport.requests[3].body, b'\x00\xff')
        self.assertEqual([response.body for response in responses], [b'', b'\x00\xff', b'\xff\x00'])
        transport.assert_exhausted()

    async def test_concurrent_sdk_calls_share_one_locked_callback_queue(self):
        transport = ScriptedHttpClient(expected_service_calls(
            (b'parallel', b'first'),
            (b'parallel', b'second'),
        ))
        sdk = client(transport)
        first, second = await asyncio.gather(
            sdk.service_request(offline_sandbox(), 'mcp', '/mcp', service_request(b'parallel')),
            sdk.service_request(offline_sandbox(), 'mcp', '/mcp', service_request(b'parallel')),
        )
        self.assertEqual({first.body, second.body}, {b'first', b'second'})
        self.assertEqual(len(transport.requests), 3)
        transport.assert_exhausted()

if __name__ == '__main__': unittest.main()

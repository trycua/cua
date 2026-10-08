"""Contract tests for the actual Driver classes; no transport/provider execution."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run import Driver, DriverToolError

class Session:
    def __init__(self, result):
        self.result, self.calls = result, []
    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        return self.result

class DriverRefusalTest(unittest.IsolatedAsyncioTestCase):
    async def check_refusal(self, flag, data, code=None, recommended=None):
        session = Session(SimpleNamespace(isError=flag, structuredContent=data, content=[]))
        with self.assertRaises(DriverToolError) as caught:
            await Driver(session, 'owned').call('browser_click', {'session': 'foreign', 'ref': 'p1:0'})
        self.assertEqual(caught.exception.code, code)
        self.assertEqual(caught.exception.recommended_delivery, recommended)
        self.assertEqual(session.calls, [('browser_click', {'session': 'owned', 'ref': 'p1:0'})])

    async def test_effect_refused_without_error_flag(self):
        await self.check_refusal(None, {'effect': 'refused', 'code': 'browser_ref_stale'}, 'browser_ref_stale')
    async def test_effect_refused_with_false_error_flag(self):
        await self.check_refusal(False, {'effect': 'refused', 'code': 'browser_binding_stale'}, 'browser_binding_stale')
    async def test_effect_refused_without_optional_metadata(self):
        await self.check_refusal(False, {'effect': 'refused'})
    async def test_effect_refused_preserves_escalation(self):
        await self.check_refusal(False, {'effect': 'refused', 'code': 'background_unavailable', 'escalation': {'recommended': 'foreground'}}, 'background_unavailable', 'foreground')
    async def test_effect_refused_nested_code_preserves_escalation(self):
        await self.check_refusal(False, {'effect': 'refused', 'refusal': {'code': 'background_unsupported'}, 'escalation': {'recommended': 'foreground'}}, 'background_unsupported', 'foreground')
    async def test_effect_refused_ignores_nonstring_metadata(self):
        await self.check_refusal(False, {'effect': 'refused', 'code': 17, 'escalation': {'recommended': ['foreground']}})
    async def test_effect_refused_takes_precedence_over_legacy_ok(self):
        await self.check_refusal(False, {'effect': 'refused', 'status': 'ok', 'code': 'permission_denied'}, 'permission_denied')
    async def test_mcp_error_still_raises(self):
        await self.check_refusal(True, {'code': 'invalid_arguments'}, 'invalid_arguments')
    async def test_mcp_error_without_structured_content(self):
        await self.check_refusal(True, None)
    async def test_legacy_refusal_still_raises(self):
        await self.check_refusal(False, {'refusal': {'code': 'denied'}}, 'denied')
    async def test_legacy_status_still_raises(self):
        await self.check_refusal(False, {'status': 'refused'})
    async def test_confirmed_returned_unchanged(self):
        data = {'effect': 'confirmed', 'result': {'count': 1}}
        session = Session(SimpleNamespace(isError=False, structuredContent=data))
        self.assertIs(await Driver(session, 'owned').call('browser_click', {}), data)
        self.assertEqual(len(session.calls), 1)
    async def test_unverifiable_is_not_refusal_or_retry_permission(self):
        data = {'effect': 'unverifiable', 'escalation': {'recommended': 'verify_state'}}
        session = Session(SimpleNamespace(isError=False, structuredContent=data))
        self.assertIs(await Driver(session, 'owned').call('browser_click', {}), data)
        self.assertEqual(len(session.calls), 1)
    async def test_observation_returned_unchanged(self):
        data = {'windows': []}
        session = Session(SimpleNamespace(isError=False, structuredContent=data))
        self.assertIs(await Driver(session, 'owned').call('list_windows', {}), data)
    async def test_missing_structured_result_still_raises(self):
        session = Session(SimpleNamespace(isError=False, structuredContent=None))
        with self.assertRaisesRegex(RuntimeError, 'no structured result'):
            await Driver(session, 'owned').call('list_windows', {})

if __name__ == '__main__':
    unittest.main()

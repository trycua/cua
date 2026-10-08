"""Contract tests for the actual Driver classes; no transport/provider execution."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run import Driver, DriverToolError, background_refusal_code

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

class LegacyRefusalMetadataTest(unittest.IsolatedAsyncioTestCase):
    async def error_for(self, data, *, flag=False):
        session = Session(SimpleNamespace(isError=flag, structuredContent=data, content=[]))
        with self.assertRaises(DriverToolError) as caught:
            await Driver(session, "owned").call("click", {"delivery_mode": "background"})
        self.assertEqual(len(session.calls), 1, "normalization must not dispatch again")
        return caught.exception

    async def test_status_refusal_preserves_top_level_code_and_escalation(self):
        error = await self.error_for({
            "status": "refused", "code": "background_unavailable",
            "escalation": {"recommended": "foreground"},
        })
        self.assertEqual((error.code, error.recommended_delivery), ("background_unavailable", "foreground"))
        self.assertIn("click refused:", str(error))

    async def test_status_refusal_preserves_nested_code_and_escalation(self):
        error = await self.error_for({
            "status": "refused", "refusal": {"code": "unsupported_delivery"},
            "escalation": {"recommended": "foreground"},
        })
        self.assertEqual((error.code, error.recommended_delivery), ("unsupported_delivery", "foreground"))

    async def test_nested_refusal_preserves_escalation_without_status(self):
        error = await self.error_for({
            "refusal": {"code": "unsupported_delivery"},
            "escalation": {"recommended": "foreground"},
        })
        self.assertEqual((error.code, error.recommended_delivery), ("unsupported_delivery", "foreground"))

    async def test_legacy_refusal_prefers_valid_top_level_code(self):
        error = await self.error_for({
            "refusal": {"code": "nested_reason"}, "code": "permission_denied",
        })
        self.assertEqual(error.code, "permission_denied")
        self.assertIsNone(error.recommended_delivery)

    async def test_status_refusal_can_recommend_foreground_without_code(self):
        error = await self.error_for({
            "status": "refused", "escalation": {"recommended": "foreground"},
        })
        self.assertIsNone(error.code)
        self.assertEqual(error.recommended_delivery, "foreground")

    async def test_legacy_non_object_refusal_keeps_valid_metadata(self):
        error = await self.error_for({
            "refusal": "unsupported", "code": "unsupported_delivery",
            "escalation": {"recommended": "foreground"},
        })
        self.assertEqual((error.code, error.recommended_delivery), ("unsupported_delivery", "foreground"))

    async def test_invalid_top_level_code_uses_nested_code_on_every_error_path(self):
        for flag, marker in ((True, {}), (False, {"effect": "refused"}), (False, {"status": "refused"})):
            with self.subTest(flag=flag, marker=marker):
                error = await self.error_for({**marker, "code": 17, "refusal": {"code": "background_unavailable"}}, flag=flag)
                self.assertEqual(error.code, "background_unavailable")

    async def test_empty_top_level_code_falls_back_to_nested_code(self):
        error = await self.error_for({
            "status": "refused", "code": "", "refusal": {"code": "unsupported_delivery"},
            "escalation": {"recommended": "foreground"},
        })
        self.assertEqual((error.code, error.recommended_delivery), ("unsupported_delivery", "foreground"))

    async def test_malformed_legacy_metadata_is_not_used(self):
        error = await self.error_for({
            "status": "refused", "code": 17, "refusal": {"code": []},
            "escalation": {"recommended": ["foreground"]},
        })
        self.assertIsNone(error.code)
        self.assertIsNone(error.recommended_delivery)

    async def test_legacy_metadata_reaches_existing_background_recovery(self):
        error = await self.error_for({
            "status": "refused", "code": "unsupported_delivery",
            "escalation": {"recommended": "foreground"},
        })
        candidate = SimpleNamespace(tool="click", arguments={"delivery_mode": "background"})
        self.assertEqual(background_refusal_code(candidate, error), "unsupported_delivery")

    async def test_recommendation_without_code_keeps_existing_recovery_reason(self):
        error = await self.error_for({
            "status": "refused", "escalation": {"recommended": "foreground"},
        })
        candidate = SimpleNamespace(tool="click", arguments={"delivery_mode": "background"})
        self.assertEqual(background_refusal_code(candidate, error), "foreground_recommended")

    async def test_recovery_stays_limited_to_background_click(self):
        error = await self.error_for({
            "status": "refused", "code": "background_unavailable",
            "escalation": {"recommended": "foreground"},
        })
        for tool, delivery in (("click", "foreground"), ("type_text", "background"), ("browser_click", "background")):
            with self.subTest(tool=tool, delivery=delivery):
                candidate = SimpleNamespace(tool=tool, arguments={"delivery_mode": delivery})
                self.assertIsNone(background_refusal_code(candidate, error))

    async def test_permission_denial_is_not_foreground_permission(self):
        error = await self.error_for({"status": "refused", "code": "permission_denied"})
        candidate = SimpleNamespace(tool="click", arguments={"delivery_mode": "background"})
        self.assertEqual(error.code, "permission_denied")
        self.assertIsNone(background_refusal_code(candidate, error))

    async def test_recommendation_alone_does_not_turn_observation_into_refusal(self):
        data = {"windows": [], "escalation": {"recommended": "foreground"}}
        session = Session(SimpleNamespace(isError=False, structuredContent=data))
        self.assertIs(await Driver(session, "owned").call("list_windows", {}), data)


if __name__ == '__main__':
    unittest.main()

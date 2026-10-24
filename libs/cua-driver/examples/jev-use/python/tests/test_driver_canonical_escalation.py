"""Read canonical ActionResult metadata without changing recovery authority."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from run import Driver, DriverToolError, background_refusal_code


class CanonicalEscalationTest(unittest.IsolatedAsyncioTestCase):
    async def check(self, data, *, flag=False, code=None, recommended=None,
                    recovery=None, tool="click", delivery="background"):
        calls = []

        async def call_tool(name, arguments):
            calls.append((name, arguments))
            return SimpleNamespace(isError=flag, structuredContent=data, content=[])

        driver = Driver(SimpleNamespace(call_tool=call_tool), "owned")
        args = {"session": "foreign", "delivery_mode": delivery}
        candidate = SimpleNamespace(tool=tool, arguments=args)
        with self.assertRaises(DriverToolError) as caught:
            await driver.call(tool, args)
        self.assertEqual(caught.exception.code, code)
        self.assertEqual(caught.exception.recommended_delivery, recommended)
        self.assertEqual(background_refusal_code(candidate, caught.exception), recovery)
        self.assertEqual(calls, [(tool, {**args, "session": "owned"})])
        self.assertEqual(args["session"], "foreign")

    async def test_canonical_error_code(self):
        await self.check({'effect': 'refused', 'route': 'dom', 'error': {'code': 'browser_ref_stale'}}, code='browser_ref_stale', tool='browser_click')

    async def test_canonical_foreground_target(self):
        await self.check({'effect': 'refused', 'route': 'accessibility', 'error': {'code': 'element_disabled'}, 'escalation': {'target': 'foreground', 'reason': 'route_unavailable'}}, code='element_disabled', recommended='foreground', recovery='element_disabled')

    async def test_canonical_code_precedes_legacy_code(self):
        await self.check({'effect': 'refused', 'error': {'code': 'canonical'}, 'code': 'legacy', 'refusal': {'code': 'nested'}}, code='canonical')

    async def test_malformed_canonical_code_falls_back(self):
        await self.check({'effect': 'refused', 'error': {'code': 17}, 'code': 'background_unavailable'}, code='background_unavailable', recovery='background_unavailable')

    async def test_mcp_error_reads_canonical_fields(self):
        await self.check({'error': {'code': 'route_unavailable'}, 'escalation': {'target': 'foreground', 'reason': 'route_unavailable'}}, flag=True, code='route_unavailable', recommended='foreground', recovery='route_unavailable')

    async def test_canonical_session_overrides_legacy_foreground(self):
        await self.check({'effect': 'refused', 'escalation': {'target': 'session', 'reason': 'permission_required', 'recommended': 'foreground'}}, recommended='session')

    async def test_canonical_pixel_is_not_foreground(self):
        await self.check({'effect': 'refused', 'escalation': {'target': 'pixel', 'reason': 'route_unavailable'}}, recommended='pixel')

    async def test_canonical_page_is_not_foreground(self):
        await self.check({'effect': 'refused', 'escalation': {'target': 'page', 'reason': 'route_unavailable'}}, recommended='page')

    async def test_invalid_canonical_target_does_not_fall_back(self):
        await self.check({'effect': 'refused', 'escalation': {'target': None, 'recommended': 'foreground'}})

    async def test_absent_canonical_target_keeps_legacy_hint(self):
        await self.check({'effect': 'refused', 'escalation': {'recommended': 'foreground'}}, recommended='foreground', recovery='foreground_recommended')

    async def test_canonical_hint_does_not_escalate_non_click(self):
        await self.check({'effect': 'refused', 'escalation': {'target': 'foreground', 'reason': 'route_unavailable'}}, tool='type_text', recommended='foreground')

    async def test_canonical_hint_does_not_escalate_foreground_click(self):
        await self.check({'effect': 'refused', 'escalation': {'target': 'foreground', 'reason': 'route_unavailable'}}, delivery='foreground', recommended='foreground')

    async def test_canonical_permission_refusal_requires_session(self):
        await self.check({'effect': 'refused', 'error': {'code': 'permission_denied'}, 'escalation': {'target': 'session', 'reason': 'permission_required'}}, code='permission_denied', recommended='session')

    async def test_unverifiable_target_is_not_a_refusal_or_retry(self):
        data = {"effect": "unverifiable", "escalation": {"target": "foreground", "reason": "effect_unconfirmed"}}
        calls = []
        async def call_tool(name, arguments):
            calls.append((name, arguments))
            return SimpleNamespace(isError=False, structuredContent=data, content=[])
        driver = Driver(SimpleNamespace(call_tool=call_tool), "owned")
        self.assertIs(await driver.call("click", {"delivery_mode": "background"}), data)
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()

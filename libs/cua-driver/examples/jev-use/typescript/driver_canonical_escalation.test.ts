import assert from 'node:assert/strict';
import test from 'node:test';
import { Driver, DriverToolError, backgroundRefusalCode } from './run.js';

type Case = { name: string; data: Record<string, unknown>; flag?: boolean;
  code?: string; recommended?: string; recovery?: string; tool?: string; delivery?: string };
const cases: Case[] = [
  {
    "name": "canonical_error_code",
    "data": {
      "effect": "refused",
      "route": "dom",
      "error": {
        "code": "browser_ref_stale"
      }
    },
    "code": "browser_ref_stale",
    "tool": "browser_click"
  },
  {
    "name": "canonical_foreground_target",
    "data": {
      "effect": "refused",
      "route": "accessibility",
      "error": {
        "code": "element_disabled"
      },
      "escalation": {
        "target": "foreground",
        "reason": "route_unavailable"
      }
    },
    "code": "element_disabled",
    "recommended": "foreground",
    "recovery": "element_disabled"
  },
  {
    "name": "canonical_code_precedes_legacy_code",
    "data": {
      "effect": "refused",
      "error": {
        "code": "canonical"
      },
      "code": "legacy",
      "refusal": {
        "code": "nested"
      }
    },
    "code": "canonical"
  },
  {
    "name": "malformed_canonical_code_falls_back",
    "data": {
      "effect": "refused",
      "error": {
        "code": 17
      },
      "code": "background_unavailable"
    },
    "code": "background_unavailable",
    "recovery": "background_unavailable"
  },
  {
    "name": "mcp_error_reads_canonical_fields",
    "data": {
      "error": {
        "code": "route_unavailable"
      },
      "escalation": {
        "target": "foreground",
        "reason": "route_unavailable"
      }
    },
    "flag": true,
    "code": "route_unavailable",
    "recommended": "foreground",
    "recovery": "route_unavailable"
  },
  {
    "name": "canonical_session_overrides_legacy_foreground",
    "data": {
      "effect": "refused",
      "escalation": {
        "target": "session",
        "reason": "permission_required",
        "recommended": "foreground"
      }
    },
    "recommended": "session"
  },
  {
    "name": "canonical_pixel_is_not_foreground",
    "data": {
      "effect": "refused",
      "escalation": {
        "target": "pixel",
        "reason": "route_unavailable"
      }
    },
    "recommended": "pixel"
  },
  {
    "name": "canonical_page_is_not_foreground",
    "data": {
      "effect": "refused",
      "escalation": {
        "target": "page",
        "reason": "route_unavailable"
      }
    },
    "recommended": "page"
  },
  {
    "name": "invalid_canonical_target_does_not_fall_back",
    "data": {
      "effect": "refused",
      "escalation": {
        "target": null,
        "recommended": "foreground"
      }
    }
  },
  {
    "name": "absent_canonical_target_keeps_legacy_hint",
    "data": {
      "effect": "refused",
      "escalation": {
        "recommended": "foreground"
      }
    },
    "recommended": "foreground",
    "recovery": "foreground_recommended"
  },
  {
    "name": "canonical_hint_does_not_escalate_non_click",
    "data": {
      "effect": "refused",
      "escalation": {
        "target": "foreground",
        "reason": "route_unavailable"
      }
    },
    "tool": "type_text",
    "recommended": "foreground"
  },
  {
    "name": "canonical_hint_does_not_escalate_foreground_click",
    "data": {
      "effect": "refused",
      "escalation": {
        "target": "foreground",
        "reason": "route_unavailable"
      }
    },
    "delivery": "foreground",
    "recommended": "foreground"
  },
  {
    "name": "canonical_permission_refusal_requires_session",
    "data": {
      "effect": "refused",
      "error": {
        "code": "permission_denied"
      },
      "escalation": {
        "target": "session",
        "reason": "permission_required"
      }
    },
    "code": "permission_denied",
    "recommended": "session"
  }
];

for (const c of cases) test(c.name, async () => {
  const calls: unknown[] = [];
  const client = { callTool: async (args: unknown) => {
    calls.push(args);
    return { isError: c.flag ?? false, structuredContent: c.data, content: [] };
  } };
  const driver = new Driver(client as unknown as ConstructorParameters<typeof Driver>[0], 'owned');
  const tool = c.tool ?? 'click';
  const args = { session: 'foreign', delivery_mode: c.delivery ?? 'background' };
  const candidate = { tool, arguments: args } as unknown as Parameters<typeof backgroundRefusalCode>[0];
  await assert.rejects(driver.call(tool, args), error => {
    assert.ok(error instanceof DriverToolError);
    assert.equal(error.code, c.code);
    assert.equal(error.recommendedDelivery, c.recommended);
    assert.equal(backgroundRefusalCode(candidate, error), c.recovery);
    return true;
  });
  assert.deepEqual(calls, [{ name: tool, arguments: { ...args, session: 'owned' } }]);
  assert.equal(args.session, 'foreign');
});

test('unverifiable target is not a refusal or retry', async () => {
  const data = { effect: 'unverifiable', escalation: { target: 'foreground', reason: 'effect_unconfirmed' } };
  let calls = 0;
  const client = { callTool: async () => { calls += 1; return { isError: false, structuredContent: data }; } };
  const driver = new Driver(client as unknown as ConstructorParameters<typeof Driver>[0], 'owned');
  assert.equal(await driver.call('click', { delivery_mode: 'background' }), data);
  assert.equal(calls, 1);
});

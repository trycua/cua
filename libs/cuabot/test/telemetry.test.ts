/**
 * Offline tests for cuabot telemetry sanitizers. Run: pnpm test
 * HOME points at a temp dir; no PostHog client is created.
 */
import { strict as assert } from 'node:assert';
import { mkdtempSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, describe, it } from 'node:test';

const home = mkdtempSync(join(tmpdir(), 'cuabot-telemetry-test-'));
process.env.HOME = home;
process.env.USERPROFILE = home;
for (const name of ['DO_NOT_TRACK', 'CUA_TELEMETRY', 'CUA_TELEMETRY_ENABLED', 'CUABOT_TELEMETRY']) {
  delete process.env[name];
}

type Mod = typeof import('../src/telemetry.js');
type Settings = typeof import('../src/settings.js');
let t: Mod;
let settings: Settings;

before(async () => {
  settings = await import('../src/settings.js');
  t = await import('../src/telemetry.js');
});
after(() => rmSync(home, { recursive: true, force: true }));

describe('cliSubcommandLabel', () => {
  it('keeps known flags and agent ids', () => {
    assert.equal(t.cliSubcommandLabel(['--status']), '--status');
    assert.equal(t.cliSubcommandLabel(['claude', '--resume']), 'claude');
    assert.equal(t.cliSubcommandLabel(['--name', 'alice-secret', '--type', 'hello']), '--type');
  });
  it('maps arbitrary commands to other and empty to none', () => {
    assert.equal(t.cliSubcommandLabel(['cat', '/Users/alice/secret.txt']), 'other');
    assert.equal(t.cliSubcommandLabel(['-n', 'alice-secret']), 'none');
  });
});

describe('sanitizeRelayedEvent', () => {
  it('drops unknown event types', () => {
    assert.equal(t.sanitizeRelayedEvent({ type: 'prompt_change', prompt: 'x' }), null);
    assert.equal(t.sanitizeRelayedEvent({ type: 'anything' }), null);
    assert.equal(t.sanitizeRelayedEvent('nope'), null);
  });
  it('strips unknown properties from cli_invocation', () => {
    const ev = t.sanitizeRelayedEvent({
      type: 'cli_invocation',
      timestamp: 1,
      subcommand: '/Users/alice',
      cwd: '/Users/alice',
      cli_args: ['secret'],
    });
    assert.deepEqual(ev, { type: 'cli_invocation', timestamp: 1, subcommand: 'other' });
  });
  it('keeps only safe numeric tool args', () => {
    const ev = t.sanitizeRelayedEvent({
      type: 'mcp_tool_call',
      timestamp: 2,
      tool_name: 'type_text',
      tool_args: { text: 'my password', delay: 5, keys: 'ctrl+c', save_path: '/tmp/x' },
      extra: 'secret',
    });
    assert.deepEqual(ev, {
      type: 'mcp_tool_call',
      timestamp: 2,
      tool_name: 'type_text',
      tool_args: { delay: 5 },
    });
    assert.equal(t.sanitizeRelayedEvent({ type: 'mcp_tool_call', tool_name: 'evil' }), null);
  });
});

describe('enablement', () => {
  it('is off by default (opt-in) and creates no installation id', () => {
    assert.equal(settings.getTelemetryEnabled(), false);
    const client = new t.CuabotTelemetry();
    client.recordEvent({ type: 'startup', timestamp: 0 });
    assert.equal(existsSync(join(home, '.config', 'cua', 'installation_id')), false);
    assert.equal(existsSync(join(home, '.cua', 'installation_id')), false);
  });
  it('DO_NOT_TRACK and CUA_TELEMETRY=0 override the opt-in', () => {
    process.env.CUABOT_TELEMETRY = 'true';
    try {
      assert.equal(settings.getTelemetryEnabled(), true);
      process.env.DO_NOT_TRACK = '1';
      assert.equal(settings.getTelemetryEnabled(), false);
      delete process.env.DO_NOT_TRACK;
      process.env.CUA_TELEMETRY = '0';
      assert.equal(settings.getTelemetryEnabled(), false);
    } finally {
      delete process.env.CUABOT_TELEMETRY;
      delete process.env.DO_NOT_TRACK;
      delete process.env.CUA_TELEMETRY;
    }
  });
});

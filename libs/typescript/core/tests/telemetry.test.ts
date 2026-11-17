import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  CI_ENV_VARS,
  isTelemetryEnabledFromEnv,
  machineTelemetrySetting,
  type PostHogFactory,
  type PostHogLike,
  Telemetry,
} from '../src/';

// Never create a real PostHog client: every Telemetry in this file gets a fake.
vi.mock('posthog-node', () => ({
  PostHog: vi.fn(() => {
    throw new Error('real PostHog client must not be created in tests');
  }),
}));

const SWITCHES = [
  'DO_NOT_TRACK',
  'CUA_TELEMETRY',
  'CUA_TELEMETRY_ENABLED',
  'CUA_TELEMETRY_DISABLED',
  'CUA_HOME',
  ...CI_ENV_VARS,
];

function fakeFactory() {
  const client = {
    capture: vi.fn(),
    flush: vi.fn(async () => undefined),
    shutdown: vi.fn(async () => undefined),
    disable: vi.fn(async () => undefined),
  } satisfies PostHogLike;
  const factory = vi.fn<PostHogFactory>(() => client);
  return { client, factory };
}

describe('Telemetry', () => {
  let home: string;
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    home = fs.mkdtempSync(path.join(os.tmpdir(), 'cua-telemetry-test-'));
    for (const name of [...SWITCHES, 'HOME']) saved[name] = process.env[name];
    for (const name of SWITCHES) delete process.env[name];
    process.env.HOME = home;
  });

  afterEach(() => {
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
    fs.rmSync(home, { recursive: true, force: true });
  });

  const make = (env: Record<string, string> = {}) => {
    const { client, factory } = fakeFactory();
    const telemetry = new Telemetry({ clientFactory: factory, env, homeDir: home });
    return { telemetry, client, factory };
  };

  describe('enablement', () => {
    it('is on by default', () => {
      expect(make().telemetry.enabled).toBe(true);
    });

    it.each(['false', '0', 'no', 'off'])('legacy CUA_TELEMETRY_ENABLED=%s disables', (v) => {
      expect(make({ CUA_TELEMETRY_ENABLED: v }).telemetry.enabled).toBe(false);
    });

    it.each(['true', '1'])('legacy CUA_TELEMETRY_ENABLED=%s keeps it on', (v) => {
      expect(make({ CUA_TELEMETRY_ENABLED: v }).telemetry.enabled).toBe(true);
    });

    it.each(['0', 'false', 'no', 'off'])('CUA_TELEMETRY=%s disables', (v) => {
      expect(make({ CUA_TELEMETRY: v }).telemetry.enabled).toBe(false);
    });

    it('DO_NOT_TRACK disables, even with CUA_TELEMETRY=1', () => {
      expect(make({ DO_NOT_TRACK: '1' }).telemetry.enabled).toBe(false);
      expect(make({ DO_NOT_TRACK: 'true', CUA_TELEMETRY: '1' }).telemetry.enabled).toBe(false);
      expect(make({ DO_NOT_TRACK: '0' }).telemetry.enabled).toBe(true);
    });

    it.each(['"off"', 'false', '0', "'disabled'"])(
      'the machine setting enabled = %s in ~/.cua/config.toml disables',
      (value) => {
        fs.mkdirSync(path.join(home, '.cua'));
        fs.writeFileSync(
          path.join(home, '.cua', 'config.toml'),
          `[other]\nenabled = "on"\n\n[telemetry]\nenabled = ${value} # machine\n`
        );
        expect(make().telemetry.enabled).toBe(false);
        expect(isTelemetryEnabledFromEnv({ HOME: home })).toBe(false);
      }
    );

    it('reads the machine setting under CUA_HOME, and CUA_TELEMETRY=1 wins over it', () => {
      const custom = path.join(home, 'custom');
      fs.mkdirSync(custom);
      fs.writeFileSync(path.join(custom, 'config.toml'), 'telemetry.enabled = "off"\n');
      expect(make({ CUA_HOME: custom }).telemetry.enabled).toBe(false);
      expect(make({ CUA_HOME: custom, CUA_TELEMETRY: '1' }).telemetry.enabled).toBe(true);
      expect(machineTelemetrySetting({ CUA_HOME: custom })).toBe(false);
    });

    it('an unrelated or unreadable config keeps the default', () => {
      fs.mkdirSync(path.join(home, '.cua'));
      fs.writeFileSync(path.join(home, '.cua', 'config.toml'), '[other]\nenabled = "off"\n');
      expect(make().telemetry.enabled).toBe(true);
      expect(machineTelemetrySetting({})).toBeUndefined();
    });

    it('legacy CUA_TELEMETRY_DISABLED disables', () => {
      expect(make({ CUA_TELEMETRY_DISABLED: '1' }).telemetry.enabled).toBe(false);
    });

    it.each([...CI_ENV_VARS])('is off by default in CI (%s)', (name) => {
      expect(isTelemetryEnabledFromEnv({ [name]: 'true' })).toBe(false);
    });

    it('CUA_TELEMETRY=1 or on re-enables in CI', () => {
      expect(make({ CI: 'true', CUA_TELEMETRY: '1' }).telemetry.enabled).toBe(true);
      expect(make({ GITHUB_ACTIONS: 'true', CUA_TELEMETRY: 'on' }).telemetry.enabled).toBe(true);
    });
  });

  describe('privacy', () => {
    it('does not create the id file or a client when disabled', () => {
      const { telemetry, client, factory } = make({ CUA_TELEMETRY: '0' });
      telemetry.recordEvent('evt', { a: 1 });
      expect(factory).not.toHaveBeenCalled();
      expect(client.capture).not.toHaveBeenCalled();
      expect(fs.existsSync(path.join(home, '.config'))).toBe(false);
      expect(fs.existsSync(path.join(home, '.cua'))).toBe(false);
    });

    it('disables GeoIP and person profiles on every event', async () => {
      const { telemetry, client, factory } = make();
      telemetry.recordEvent('evt', { a: 1 });
      await telemetry.flush();

      expect(factory).toHaveBeenCalledTimes(1);
      expect(factory.mock.calls[0][1].disableGeoip).toBe(true);
      expect(client.capture).toHaveBeenCalledTimes(2); // evt + telemetry_counters
      for (const [msg] of client.capture.mock.calls) {
        expect(msg.disableGeoip).toBe(true);
        expect(msg.properties?.$process_person_profile).toBe(false);
        expect(msg.event).not.toBe('$identify');
      }
    });

    it('persists a random id under ~/.config/cua/installation_id', () => {
      const { client, telemetry } = make();
      telemetry.recordEvent('evt');
      const id = fs.readFileSync(path.join(home, '.config', 'cua', 'installation_id'), 'utf-8');
      expect(id).toMatch(/^[0-9a-f-]{36}$/);
      expect(client.capture.mock.calls[0][0].distinctId).toBe(id);
    });
  });
});

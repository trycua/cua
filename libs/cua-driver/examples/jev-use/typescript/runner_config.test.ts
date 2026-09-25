import assert from 'node:assert/strict';
import process from 'node:process';
import test from 'node:test';

import { jevConfigFromArgs } from './run.js';

const KEYS = [
  'JEV_BACKEND', 'JEV_BASE_URL', 'TYPESAFE_BASE_URL', 'OPENJEV_BASE_URL',
  'JEV_API_KEY', 'TYPESAFE_API_KEY', 'JEV_MODEL', 'TYPESAFE_MODEL',
  'JEV_TIMEOUT_MS', 'TYPESAFE_TIMEOUT_MS',
] as const;

function args(overrides: Partial<Parameters<typeof jevConfigFromArgs>[0]> = {}) {
  return { fixtureUrl: 'http://127.0.0.1:8765/', maxSteps: 4, dryRun: false, ...overrides };
}

function snapshot() {
  return Object.fromEntries(KEYS.map((key) => [key, process.env[key]]));
}

// Synchronous tests restore only the configuration keys they temporarily own.
function withEnvironment<T>(environment: Record<string, string>, action: () => T): T {
  const previous = snapshot();
  for (const key of KEYS) delete process.env[key];
  Object.assign(process.env, environment);
  const before = snapshot();
  try {
    return action();
  } finally {
    try {
      assert.deepEqual(snapshot(), before, 'runner configuration must not mutate process.env');
    } finally {
      for (const key of KEYS) {
        if (previous[key] === undefined) delete process.env[key];
        else process.env[key] = previous[key];
      }
    }
  }
}

test('omitted runner provider preserves environment backend', () => {
  withEnvironment({ JEV_BACKEND: 'local' }, () => {
    const config = jevConfigFromArgs(args());
    assert.equal(config.backend, 'local');
    assert.equal(config.baseUrl, 'http://127.0.0.1:8787');
  });
});

test('no runner selection defaults to mock', () => {
  withEnvironment({}, () => assert.equal(jevConfigFromArgs(args()).backend, 'mock'));
});

test('explicit mock overrides environment backend', () => {
  withEnvironment({ JEV_BACKEND: 'local' }, () => {
    assert.equal(jevConfigFromArgs(args({ provider: 'mock' })).backend, 'mock');
  });
});

test('live alias overrides environment backend', () => {
  withEnvironment({ JEV_BACKEND: 'local' }, () => {
    assert.equal(jevConfigFromArgs(args({ provider: 'live' })).backend, 'typesafe');
  });
});

test('runner arguments override only their environment values', () => {
  withEnvironment({
    JEV_BACKEND: 'local',
    JEV_BASE_URL: 'http://127.0.0.1:8787',
    JEV_MODEL: 'environment-model',
    JEV_TIMEOUT_MS: '1200',
    JEV_API_KEY: 'fixture-only-not-a-credential',
  }, () => {
    const config = jevConfigFromArgs(args({
      jevBaseUrl: 'http://127.0.0.1:9999', jevModel: 'cli-model', jevTimeoutMs: 750,
    }));
    assert.equal(config.backend, 'local');
    assert.equal(config.baseUrl, 'http://127.0.0.1:9999');
    assert.equal(config.model, 'cli-model');
    assert.equal(config.timeoutMs, 750);
    assert.equal(config.apiKey, 'fixture-only-not-a-credential');
  });
});

test('unknown environment backend is not silent mock', () => {
  withEnvironment({ JEV_BACKEND: 'typo' }, () => {
    assert.throws(() => jevConfigFromArgs(args()), /JEV_BACKEND/);
  });
});

test('explicit provider overrides invalid environment backend', () => {
  withEnvironment({ JEV_BACKEND: 'typo' }, () => {
    assert.equal(jevConfigFromArgs(args({ provider: 'mock' })).backend, 'mock');
  });
});

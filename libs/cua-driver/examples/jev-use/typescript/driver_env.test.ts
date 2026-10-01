import assert from 'node:assert/strict';
import test from 'node:test';

import { DESKTOP_SESSION_VARS, driverEnvironment } from './driver_env.js';

test('forwards desktop session variables when set', () => {
  const source = Object.fromEntries(DESKTOP_SESSION_VARS.map((name) => [name, `value-${name}`]));
  const env = driverEnvironment(source);
  for (const name of DESKTOP_SESSION_VARS) assert.equal(env[name], `value-${name}`);
});

test('omits desktop session variables when unset', () => {
  const saved = Object.fromEntries(DESKTOP_SESSION_VARS.map((name) => [name, process.env[name]]));
  try {
    for (const name of DESKTOP_SESSION_VARS) delete process.env[name];
    const env = driverEnvironment({ PATH: '/usr/bin' });
    for (const name of DESKTOP_SESSION_VARS) assert.equal(name in env, false, name);
  } finally {
    for (const [name, value] of Object.entries(saved)) {
      if (value !== undefined) process.env[name] = value;
    }
  }
});

test('forwards Cua Driver variables', () => {
  const env = driverEnvironment({ CUA_DRIVER_PERMISSION_MODE: 'unrestricted' });
  assert.equal(env.CUA_DRIVER_PERMISSION_MODE, 'unrestricted');
});

test('does not forward provider credentials', () => {
  const saved = process.env.TYPESAFE_API_KEY;
  process.env.TYPESAFE_API_KEY = 'secret';
  try {
    const env = driverEnvironment({ ...process.env, DISPLAY: ':99' });
    assert.equal(env.DISPLAY, ':99');
    assert.equal('TYPESAFE_API_KEY' in env, false);
    assert.equal(Object.values(env).includes('secret'), false);
  } finally {
    if (saved === undefined) delete process.env.TYPESAFE_API_KEY;
    else process.env.TYPESAFE_API_KEY = saved;
  }
});

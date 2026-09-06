import assert from 'node:assert/strict';
import test from 'node:test';

import {
  generateMCPToolDoc,
  getLatestReleasedVersion,
  selectLatestReleasedVersion,
} from './cua-driver';

test('lists matching tags through git argv without invoking a shell', () => {
  const calls: Array<{ command: string; args: readonly string[] }> = [];
  const version = getLatestReleasedVersion((command, args) => {
    calls.push({ command, args });
    return 'cua-driver-rs-v0.9.1\r\ncua-driver-rs-v0.20.0\r\n';
  });

  assert.equal(version, '0.20.0');
  assert.deepEqual(calls, [{ command: 'git', args: ['tag', '--list', 'cua-driver-rs-v*'] }]);
});

test('orders stable release tags by semantic version', () => {
  assert.equal(
    selectLatestReleasedVersion([
      'cua-driver-rs-v0.2.9',
      'cua-driver-rs-v0.20.0',
      'cua-driver-rs-v0.2.10',
      'cua-driver-rs-v0.9.1',
    ]),
    '0.20.0'
  );
});

test('ignores unrelated, nightly, prerelease, and malformed tags', () => {
  assert.equal(
    selectLatestReleasedVersion([
      'lume-v99.0.0',
      'nightly-cua-driver-rs-v99.0.0-nightly.20260816.1',
      'cua-driver-rs-v21.0.0-rc.1',
      'cua-driver-rs-v21.0',
      'prefix-cua-driver-rs-v22.0.0',
      'cua-driver-rs-v20.3.4',
    ]),
    '20.3.4'
  );
});

test('reports git failures instead of returning a placeholder version', () => {
  assert.throws(
    () =>
      getLatestReleasedVersion(() => {
        throw new Error('git is unavailable');
      }),
    /Failed to list Cua Driver release tags with git: git is unavailable/
  );
});

test('reports when git returns no valid stable release tags', () => {
  assert.throws(
    () => getLatestReleasedVersion(() => 'nightly-cua-driver-rs-v0.21.0-nightly.20260816.1\n'),
    /No stable Cua Driver release tag matching cua-driver-rs-v<major>\.<minor>\.<patch> was found/
  );
});

test('generates a valid tagged-object example for required union inputs', () => {
  const lines = generateMCPToolDoc({
    name: 'get_menu_extra_state',
    description: 'Observe a menu extra.',
    input_schema: {
      type: 'object',
      required: ['application'],
      properties: {
        application: {
          oneOf: [
            {
              type: 'object',
              required: ['kind', 'pid'],
              properties: {
                kind: { type: 'string', const: 'pid' },
                pid: { type: 'integer', minimum: 1 },
              },
            },
            {
              type: 'object',
              required: ['kind', 'bundle_id'],
              properties: {
                kind: { type: 'string', const: 'bundle_id' },
                bundle_id: { type: 'string' },
              },
            },
          ],
        },
      },
    },
  });

  assert.ok(lines.includes('{"application":{"kind":"pid","pid":844}}'));
  assert.ok(!lines.includes('{"application":"value"}'));
});

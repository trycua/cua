import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { resolveDriverBinary } from './cua-driver';

test('native policy checks use the explicit binary without a default build', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'cua-docs-binary-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = join(directory, 'scripts', 'docs-generators');
  cpSync(__dirname, source, { recursive: true });
  mkdirSync(join(directory, 'libs', 'cua-driver', 'rust'), { recursive: true });
  const result = spawnSync(
    process.execPath,
    [
      ...process.execArgv,
      '--test',
      '--test-reporter=tap',
      join(source, 'cua-driver-policy.test.ts'),
    ],
    {
      cwd: directory,
      env: {
        ...process.env,
        NODE_TEST_CONTEXT: undefined,
        CUA_DRIVER_BINARY: resolve(resolveDriverBinary()),
      },
      encoding: 'utf8',
      timeout: 60_000,
    }
  );
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /^# pass 1\r?$/m);
});

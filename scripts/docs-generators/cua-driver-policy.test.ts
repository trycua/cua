import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { extractDocumentation, type DumpDocsOutput } from './cua-driver';

test('native docs are complete while ordinary discovery remains policy-filtered', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'cua-docs-policy-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const policy = join(directory, 'policy.yaml');
  const content = 'allow:\n  tools: [get_window_state]\n';
  writeFileSync(policy, content);
  const binary = resolve(
    __dirname,
    '../../libs/cua-driver/rust/target/release',
    process.platform === 'win32' ? 'cua-driver.exe' : 'cua-driver'
  );
  const environment = {
    ...process.env,
    CUA_DRIVER_POLICY_FILE: policy,
    CUA_DRIVER_MANAGED_POLICY_FILE: policy,
  };
  const ordinary = (): DumpDocsOutput =>
    JSON.parse(
      execFileSync(binary, ['dump-docs', '--type', 'all'], { encoding: 'utf8', env: environment })
    );
  const before = ordinary();
  assert.deepEqual(
    before.mcp.tools.map((tool) => tool.name),
    ['get_window_state']
  );
  const complete = extractDocumentation(binary);
  assert.ok(complete.mcp.tools.length > before.mcp.tools.length);
  assert.deepEqual(extractDocumentation(binary, environment), complete);
  assert.deepEqual(ordinary(), before);
  assert.equal(readFileSync(policy, 'utf8'), content);
  assert.equal(environment.CUA_DRIVER_POLICY_FILE, policy);
  assert.equal(environment.CUA_DRIVER_MANAGED_POLICY_FILE, policy);
});

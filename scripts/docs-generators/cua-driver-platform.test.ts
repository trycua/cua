import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import {
  extractDocumentation,
  referencePlatform,
  syncReferences,
  type DumpDocsOutput,
} from './cua-driver';

const platforms = [
  { host: 'linux', name: 'Linux', outputFile: 'mcp-tools-linux.mdx' },
  { host: 'darwin', name: 'macOS', outputFile: 'mcp-tools.mdx' },
  { host: 'win32', name: 'Windows', outputFile: 'mcp-tools-windows.mdx' },
];

function docs(description = 'Inspect the native tree.'): DumpDocsOutput {
  return {
    cli: { name: 'cua-driver', version: '1.0.0', abstract: 'Shared CLI.', commands: [] },
    mcp: {
      version: '1.0.0',
      tools: [
        {
          name: 'get_window_state',
          description,
          input_schema: {
            type: 'object',
            required: ['pid'],
            properties: { pid: { type: 'integer', description: 'Native process.' } },
          },
        },
      ],
    },
  };
}

test('documentation extraction isolates both policy layers only in its metadata child', () => {
  const environment = Object.freeze({
    Path: 'native-toolchain',
    CARGO: 'cargo.exe',
    CUA_DRIVER_POLICY_FILE: 'restricted.yaml',
    cua_driver_policy_file: 'another-case.yaml',
    CuA_DrIvEr_MaNaGeD_PoLiCy_FiLe: 'managed.yaml',
  });
  const expected = docs();
  let calls = 0;
  const actual = extractDocumentation('native-driver', environment, (binary, args, options) => {
    calls++;
    assert.equal(binary, 'native-driver');
    assert.deepEqual(args, ['dump-docs', '--type', 'all', '--pretty']);
    assert.deepEqual(options.env, { Path: 'native-toolchain', CARGO: 'cargo.exe' });
    assert.equal(options.encoding, 'utf-8');
    return JSON.stringify(expected);
  });
  assert.equal(calls, 1);
  assert.deepEqual(actual, expected);
  assert.equal(environment.CUA_DRIVER_POLICY_FILE, 'restricted.yaml');
  assert.equal(environment.CuA_DrIvEr_MaNaGeD_PoLiCy_FiLe, 'managed.yaml');
});

test('maps native hosts and refuses unsupported hosts before generation', () => {
  for (const platform of platforms) assert.deepEqual(referencePlatform(platform.host), platform);
  assert.throws(() => referencePlatform('freebsd'), /not implemented/);
});

for (const expected of platforms) {
  test(`${expected.host} generation and checking own only the native MCP and shared CLI files`, (t) => {
    const dir = mkdtempSync(join(tmpdir(), 'cua-docs-test-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const platform = referencePlatform(expected.host);
    const others = platforms.filter((other) => other.host !== expected.host);
    writeFileSync(join(dir, 'mcp-tool-notes.mdx'), 'Shared guidance.');
    for (const other of others) writeFileSync(join(dir, other.outputFile), 'Other platform.');
    assert.deepEqual(syncReferences(dir, docs(), '1.0.0', platform, true), [
      'cli-reference.mdx',
      expected.outputFile,
    ]);
    assert.equal(readdirSync(dir).length, platforms.length);
    syncReferences(dir, docs(), '1.0.0', platform, false);
    assert.deepEqual(syncReferences(dir, docs(), '1.0.0', platform, false), []);
    assert.deepEqual(syncReferences(dir, docs(), '1.0.0', platform, true), []);
    assert.equal(readFileSync(join(dir, 'mcp-tool-notes.mdx'), 'utf8'), 'Shared guidance.');
    const before = readFileSync(join(dir, expected.outputFile), 'utf8');
    assert.ok(before.includes(`title: MCP Tools (${expected.name})`));
    for (const other of others) {
      assert.ok(before.includes(`/reference/cua-driver/${other.outputFile.replace('.mdx', '')}`));
    }
    assert.ok(before.includes('/reference/cua-driver/mcp-tool-notes'));
    assert.ok(before.includes('### `get_window_state`'));
    assert.ok(before.includes('- `pid` (integer, required): Native process.'));
    assert.deepEqual(
      syncReferences(dir, docs('Updated native description.'), '1.0.0', platform, true),
      [expected.outputFile]
    );
    assert.equal(readFileSync(join(dir, expected.outputFile), 'utf8'), before);
    syncReferences(dir, docs('Updated native description.'), '1.0.0', platform, false);
    assert.match(
      readFileSync(join(dir, expected.outputFile), 'utf8'),
      /Updated native description/
    );
    for (const other of others)
      assert.equal(readFileSync(join(dir, other.outputFile), 'utf8'), 'Other platform.');
    assert.deepEqual(
      readdirSync(dir).sort(),
      ['cli-reference.mdx', 'mcp-tool-notes.mdx', ...platforms.map((p) => p.outputFile)].sort()
    );
  });
}

test('shared CLI rendering does not depend on native MCP definitions or platform', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'cua-docs-parity-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  syncReferences(dir, docs('AT-SPI tree.'), '1.0.0', referencePlatform('linux'), false);
  const cli = readFileSync(join(dir, 'cli-reference.mdx'), 'utf8');
  for (const platform of platforms.filter((p) => p.host !== 'linux')) {
    assert.deepEqual(
      syncReferences(dir, docs(platform.name), '1.0.0', referencePlatform(platform.host), false),
      [platform.outputFile]
    );
    assert.equal(readFileSync(join(dir, 'cli-reference.mdx'), 'utf8'), cli);
  }
});

import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { referencePlatform, syncReferences, type DumpDocsOutput } from './cua-driver';

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

test('maps native hosts and refuses unsupported hosts before generation', () => {
  assert.equal(referencePlatform('linux'), 'linux');
  assert.equal(referencePlatform('darwin'), 'macos');
  for (const host of ['win32', 'freebsd']) {
    assert.throws(() => referencePlatform(host), /not implemented/);
  }
});

for (const platform of ['linux', 'macos'] as const) {
  test(`${platform} generation and checking own only the native MCP and shared CLI files`, (t) => {
    const dir = mkdtempSync(join(tmpdir(), 'cua-docs-test-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const ownFile = platform === 'linux' ? 'mcp-tools-linux.mdx' : 'mcp-tools.mdx';
    const otherFile = platform === 'linux' ? 'mcp-tools.mdx' : 'mcp-tools-linux.mdx';
    writeFileSync(join(dir, 'mcp-tool-notes.mdx'), 'Shared guidance.');
    writeFileSync(join(dir, otherFile), 'Other platform.');
    assert.deepEqual(syncReferences(dir, docs(), '1.0.0', platform, true), [
      'cli-reference.mdx',
      ownFile,
    ]);
    assert.equal(readdirSync(dir).length, 2);
    syncReferences(dir, docs(), '1.0.0', platform, false);
    assert.deepEqual(syncReferences(dir, docs(), '1.0.0', platform, false), []);
    assert.deepEqual(syncReferences(dir, docs(), '1.0.0', platform, true), []);
    assert.equal(readFileSync(join(dir, 'mcp-tool-notes.mdx'), 'utf8'), 'Shared guidance.');
    assert.equal(readFileSync(join(dir, otherFile), 'utf8'), 'Other platform.');
    const before = readFileSync(join(dir, ownFile), 'utf8');
    assert.ok(before.includes(`title: MCP Tools (${platform === 'linux' ? 'Linux' : 'macOS'})`));
    assert.ok(before.includes(`/reference/cua-driver/${otherFile.replace('.mdx', '')}`));
    assert.ok(before.includes('/reference/cua-driver/mcp-tool-notes'));
    assert.ok(before.includes('### `get_window_state`'));
    assert.ok(before.includes('- `pid` (integer, required): Native process.'));
    assert.deepEqual(
      syncReferences(dir, docs('Updated native description.'), '1.0.0', platform, true),
      [ownFile]
    );
    assert.equal(readFileSync(join(dir, ownFile), 'utf8'), before);
    syncReferences(dir, docs('Updated native description.'), '1.0.0', platform, false);
    assert.match(readFileSync(join(dir, ownFile), 'utf8'), /Updated native description/);
    assert.equal(readFileSync(join(dir, otherFile), 'utf8'), 'Other platform.');
    assert.deepEqual(readdirSync(dir).sort(), [
      'cli-reference.mdx',
      'mcp-tool-notes.mdx',
      'mcp-tools-linux.mdx',
      'mcp-tools.mdx',
    ]);
  });
}

test('shared CLI rendering does not depend on native MCP definitions or platform', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'cua-docs-parity-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  syncReferences(dir, docs('AT-SPI tree.'), '1.0.0', 'linux', false);
  const cli = readFileSync(join(dir, 'cli-reference.mdx'), 'utf8');
  assert.deepEqual(syncReferences(dir, docs('AX tree.'), '1.0.0', 'macos', false), [
    'mcp-tools.mdx',
  ]);
  assert.equal(readFileSync(join(dir, 'cli-reference.mdx'), 'utf8'), cli);
});

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import test from 'node:test';
import { SOURCES, errorKinds, paramsTable, readProtocol, renderAll, statedDefault, type Manifest, type Tool } from './spaces';

const read = (p: string) => fs.readFileSync(p, 'utf-8');
const manifest = (): Manifest => JSON.parse(read(SOURCES.manifest));

test('the checked-in contract renders one page per category, plus index and errors', () => {
  const m = manifest();
  const files = renderAll(m, readProtocol(read(SOURCES.mcp), read(SOURCES.cliMcp)));
  const names = [...files.keys()].map((f) => f.split('/').pop());
  assert.deepEqual(names, ['index.mdx', ...m.categories.map((c) => `${c.id}.mdx`), 'errors.mdx', 'meta.json']);
  // Every tool is documented exactly once, under its category.
  for (const t of m.tools) {
    const page = [...files].find(([f]) => f.endsWith(`/${t.category}.mdx`))![1];
    assert.equal(page.split(`\n## ${t.name}\n`).length, 2, t.name);
  }
  for (const [, content] of files) assert.ok(content.length < 150_000);
});

test('protocol facts are read from the server and the CLI', () => {
  const p = readProtocol(read(SOURCES.mcp), read(SOURCES.cliMcp));
  assert.deepEqual(
    p.codes.map((c) => c.code),
    [-32700, -32600, -32601, -32602]
  );
  assert.ok(p.codes.every((c) => c.doc.length > 0), 'every JSON-RPC code is documented at the source');
  assert.throws(() => readProtocol('pub mod codes {\n}', ''), /no JSON-RPC codes/);
});

test('error kinds combine the implied and the tool-specific ones', () => {
  const tool = {
    name: 't',
    requires: ['hotspot'],
    host_requires: [],
    errors: ['env'],
    input_schema: { type: 'object', properties: { space: { type: 'string' } } },
  } as unknown as Tool;
  assert.deepEqual(errorKinds(tool), [
    'invalid_argument',
    'not_found',
    'ambiguous_sandbox',
    'spacesd_not_available',
    'capability_missing',
    'env',
  ]);
});

test('parameters show required, stated defaults and const alternatives', () => {
  const rows = paramsTable({
    type: 'object',
    required: ['space'],
    properties: {
      space: { type: 'string', description: 'Space id or name.' },
      timeout: { type: 'integer', description: 'Seconds. Default 60.' },
      runtime: {
        description: 'Runtime.',
        oneOf: [
          { const: 'kubevirt', type: 'string', description: 'A VM.' },
          { const: 'gvisor', type: 'string', description: 'A pod.' },
        ],
      },
    },
  }).join('\n');
  assert.match(rows, /\| `space` \| `string` \| required \|/);
  assert.match(rows, /\| `timeout` \| `integer` \| `60` \|/);
  assert.match(rows, /\| `runtime` \| `"kubevirt" \\\| "gvisor"` \| none \| Runtime\. `kubevirt`: A VM\. `gvisor`: A pod\. \|/);
  assert.equal(statedDefault('Display name. Default: the guest hostname.'), undefined);
  assert.equal(statedDefault('Default `mcp`.'), '`mcp`');
});

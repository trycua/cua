import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { cliReference, mcpReference, renderAll, spacesAnchors, type McpDocumentation } from './cua-cli';
import type { CLIDocumentation } from './lib/cli-mdx';
import { renderCliReference } from './lib/cli-mdx';
import { codeFence, escapeMdxText, inlineCode, renderPage, slug, tabs } from './lib/mdx';

const spec: CLIDocumentation = JSON.parse(readFileSync(join(__dirname, 'cli-specs', 'cua.json'), 'utf-8'));

const mcp: McpDocumentation = {
  version: '9.9.9',
  contract_version: '0.2.0',
  mcp_protocol_version: '2025-06-18',
  permission_groups: { sandbox: { all: ['sandbox:list'], readonly: ['sandbox:list'] } },
  tools: [
    {
      name: 'add_space',
      group: 'spaces',
      permission: 'spaces:add_space',
      description: 'Register a Space.',
      instructions: 'Stored in ~/.cua.',
      providers: ['fleet', 'local'],
      metering: 'free',
      annotations: { read_only: false, destructive: false, idempotent: true, open_world: true },
      sdk_symbol: 'SpacesConnection.addSpace(url:token:)',
      input_schema: {
        type: 'object',
        properties: { url: { type: 'string', description: 'Space URL' }, tags: { type: ['array', 'null'], items: { type: 'string' } } },
        required: ['url'],
      },
    },
    { name: 'agent_start', group: 'spaces', permission: 'spaces:agent_start', description: 'Start an agent.', input_schema: { type: 'object', properties: {} } },
    { name: 'space_bash', group: 'spaces', permission: 'spaces:space_bash', description: 'Run bash.', input_schema: { type: 'object', properties: {} } },
    { name: 'sandbox_list', group: 'sandbox', permission: 'sandbox:list', description: 'List sandboxes.', input_schema: { type: 'object', properties: {} } },
  ],
};

test('every cua command is on exactly one group page', () => {
  // Throws when a visible top-level command is ungrouped or grouped twice.
  const files = renderCliReference(cliReference(spec));
  assert.ok(files.has('cli/sandbox.mdx'));
  assert.ok(files.get('cli/sandbox.mdx')!.includes('## `cua sandbox create`'));
  assert.ok(files.get('cli/index.mdx')!.includes('## Exit codes'));
});

const spaces = spacesAnchors({
  categories: [
    { id: 'lifecycle', title: 'Space lifecycle' },
    { id: 'agents', title: 'Agents' },
    { id: 'commands-and-files', title: 'Commands and files' },
  ],
  tools: [
    { name: 'add_space', category: 'lifecycle' },
    { name: 'agent_start', category: 'agents' },
    { name: 'space_bash', category: 'commands-and-files' },
  ],
});

test('Spaces tools link to their one entry in the Spaces reference instead of repeating it', () => {
  const ref = mcpReference(mcp, spaces);
  const add = ref.tools.find((t) => t.name === 'add_space')!;
  assert.deepEqual(add.facts, [
    ['Providers', 'fleet, local'],
    ['Metering', 'free'],
    ['SDK', '`SpacesConnection.addSpace(url:token:)`'],
  ]);
  const files = renderAll({ cli: spec, mcp }, spaces);
  const page = (rel: string) => [...files].find(([p]) => p.endsWith(join('reference', ...rel.split('/'))))?.[1];
  const spacesPage = page('mcp-tools/spaces.mdx')!;
  assert.ok(spacesPage.includes('| <span id="add_space"></span>[`add_space`](/spaces/reference/lifecycle#add_space) | Register a Space. |'));
  assert.ok(spacesPage.includes('[`space_bash`](/spaces/reference/commands-and-files#space_bash)'));
  assert.ok(!spacesPage.includes('| Parameter |'), 'no repeated parameter tables');
  assert.ok(spacesPage.includes('## Space lifecycle\n\nFull entries: [Space lifecycle](/spaces/reference/lifecycle).'));
  assert.equal(page('mcp-tools/space-io.mdx'), undefined);
  assert.equal(page('mcp-tools/space-agents.mdx'), undefined);
  const index = page('mcp-tools/index.mdx')!;
  assert.ok(index.includes('[`agent_start`](/spaces/reference/agents#agent_start)'));
  assert.ok(index.includes('## Permissions') && index.includes('`sandbox:all`'));
  const sandbox = page('mcp-tools/sandbox.mdx')!;
  assert.ok(sandbox.includes('## `sandbox_list`'));
  assert.throws(() => renderAll({ cli: spec, mcp }, { anchors: new Map(), sections: new Map() }), /Spaces contract manifest lacks/);
});

test('mdx helpers', () => {
  assert.equal(escapeMdxText('a <b> {c} `<d>`'), 'a &lt;b&gt; &#123;c&#125; `<d>`');
  assert.equal(inlineCode('a`b'), '``a`b``');
  assert.equal(codeFence('md', '```x```'), '````md\n```x```\n````');
  assert.equal(slug('cua sandbox create'), 'cua-sandbox-create');
  assert.ok(tabs([['Python', 'a'], ['Swift', 'b']], 'g').startsWith(`<Tabs items={['Python', 'Swift']} groupId="g" persist>`));
  const page = renderPage({ title: 'T', description: 'D', generator: 'g', source: 's', components: ['Tabs', 'Callout'], body: 'x — y' });
  assert.ok(page.includes("import { Callout } from 'fumadocs-ui/components/callout';\nimport { Tab, Tabs }"));
  assert.ok(page.endsWith('x: y\n'));
});

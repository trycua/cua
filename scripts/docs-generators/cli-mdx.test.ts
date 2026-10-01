import assert from 'node:assert/strict';
import test from 'node:test';
import {
  type CLIDocumentation,
  type CliReference,
  displayType,
  renderCliReference,
  splitExamples,
} from './lib/cli-mdx';
import { exampleArguments, renderMcpReference, schemaType, validate, type McpReference } from './lib/mcp-mdx';
import { GENERATED_MARKER, slug } from './lib/mdx';

/** Every anchor a page defines: heading slugs and `<span id>`s. */
export function anchors(page: string): string[] {
  const ids: string[] = [];
  let fenced = false;
  for (const line of page.split('\n')) {
    if (/^`{3,}/.test(line)) fenced = !fenced;
    if (fenced) continue;
    const h = line.match(/^#{1,6} (.+)$/);
    if (h) ids.push(slug(h[1].replace(/`/g, '')));
    for (const m of line.matchAll(/<span id="([^"]+)"><\/span>/g)) ids.push(m[1]);
  }
  return ids;
}

export function assertUniqueAnchors(name: string, page: string): void {
  const ids = anchors(page);
  const dupes = ids.filter((id, i) => ids.indexOf(id) !== i);
  assert.deepEqual(dupes, [], `${name} has duplicate anchors`);
}

const cli: CLIDocumentation = {
  name: 'demo',
  version: '1.2.3',
  abstract: 'A demo CLI',
  exit_codes: [
    { code: 0, meaning: 'Success' },
    { code: 2, meaning: 'Usage error' },
  ],
  global_options: [{ name: 'json', help: 'Print JSON', default_value: false, takes_value: false }],
  commands: [
    {
      name: 'vm',
      abstract: 'Virtual machines {local}',
      aliases: ['v'],
      arguments: [],
      options: [],
      flags: [],
      default_subcommand: 'ls',
      subcommands: [
        {
          name: 'create',
          abstract: 'Create a VM',
          arguments: [{ name: 'name', help: 'VM <name>', type: 'String', is_optional: false }],
          options: [
            { name: 'cpu', short_name: 'c', help: 'vCPUs', type: 'Int', default_value: '4', is_optional: true },
            { name: 'tags', help: 'Tags | labels', type: '[String]', is_optional: true, multiple_values: true },
            { name: 'on', help: 'Where', type: 'local | cloud', is_optional: false, env: 'DEMO_ON' },
          ],
          flags: [{ name: 'force', help: 'Overwrite', default_value: false }],
          subcommands: [],
          after_help: 'Examples:\n  # Make one\n  demo vm create dev\n  demo vm create dev --cpu 8  # bigger',
        },
        { name: 'ls', abstract: 'List VMs', arguments: [], options: [], flags: [], subcommands: [] },
        { name: 'secret', abstract: 'internal', hidden: true, arguments: [], options: [], flags: [], subcommands: [] },
      ],
    },
    { name: 'up', abstract: 'Update', arguments: [], options: [], flags: [], subcommands: [], examples: [{ command: 'demo up' }] },
    { name: 'dump-docs', abstract: 'internal', hidden: true, arguments: [], options: [], flags: [], subcommands: [] },
  ],
};

const ref: CliReference = {
  product: 'demo',
  cli,
  generator: 'gen',
  source: 'src',
  groups: [
    { slug: 'vm', title: 'demo vm', summary: 'Manage VMs', commands: ['vm'] },
    { slug: 'misc', title: 'Other', summary: 'Everything else', commands: ['up'] },
  ],
};

test('CLI reference: index, one page per group, nav', () => {
  const files = renderCliReference(ref);
  assert.deepEqual([...files.keys()], ['cli/index.mdx', 'cli/vm.mdx', 'cli/misc.mdx', 'cli/meta.json']);
  assert.deepEqual(JSON.parse(files.get('cli/meta.json')!).pages, ['index', 'vm', 'misc']);
  for (const [name, page] of files) {
    if (!name.endsWith('.mdx')) continue;
    assert.ok(page.includes(GENERATED_MARKER), name);
    assert.ok(page.includes('Version: 1.2.3'), name);
    assert.ok(!page.includes('secret') && !page.includes('dump-docs'), `${name} shows a hidden command`);
    assert.ok(!/undefined|\[object Object\]|—/.test(page), `${name} renders a placeholder or an em dash`);
    assertUniqueAnchors(name, page);
  }
});

test('CLI index keeps old single-page anchors and lists exit codes', () => {
  const index = renderCliReference(ref).get('cli/index.mdx')!;
  assert.ok(index.includes('### `demo vm`'));
  assert.ok(index.includes('<span id="demo-vm-create"></span>[`demo vm create`](/demo/reference/cli/vm#demo-vm-create)'));
  // The group heading already defines `demo-vm`: no second span for it.
  assert.ok(!index.includes('<span id="demo-vm"></span>'));
  assert.ok(index.includes('| `0` | Success. |'));
  assert.ok(index.includes('| `--json` | boolean | `false` | Print JSON. |'));
});

test('CLI group page: synopsis, typed flags with anchors, examples for the cli-shape lane', () => {
  const page = renderCliReference(ref).get('cli/vm.mdx')!;
  assert.ok(page.includes('demo vm [COMMAND]'));
  assert.ok(page.includes('Without a subcommand, runs `demo vm ls`.'));
  assert.ok(page.includes('Alias: `demo v`.'));
  assert.ok(page.includes('## `demo vm create`'));
  assert.ok(page.includes('demo vm create [OPTIONS] <name>'));
  assert.ok(page.includes('| `<name>` | string | required | VM &lt;name&gt;. |'));
  assert.ok(page.includes('| Flag | Short | Type | Default | Env var | Description |'));
  assert.ok(page.includes('| <span id="demo-vm-create--cpu"></span>`--cpu` | `-c` | integer | `4` |  | vCPUs. |'));
  assert.ok(page.includes('`--tags` |  | string |  |  | Tags \\| labels. Takes one or more values. |'));
  assert.ok(page.includes('| `local` \\| `cloud` | required | `DEMO_ON` |'));
  assert.ok(page.includes('`--force` |  | boolean | `false` |'));
  assert.ok(page.includes('```bash test="cli-shape" id="ref-demo-vm-create"\n# Make one\ndemo vm create dev\n\n# bigger\ndemo vm create dev --cpu 8\n```'));
  assert.ok(page.includes('Virtual machines &#123;local&#125;.'));
  assert.ok(page.includes('## Exit codes'));
  const misc = renderCliReference(ref).get('cli/misc.mdx')!;
  // A single-command page has no heading for its own command.
  assert.ok(!misc.includes('## `demo up`') && misc.includes('```bash test="cli-shape" id="ref-demo-up"\ndemo up\n```'));
});

test('a command split over pages: the first has the command, the rest its subcommands', () => {
  const split: CliReference = {
    ...ref,
    groups: [
      { slug: 'vm', title: 'demo vm', summary: 'Make VMs', commands: ['vm'], subcommands: ['create'] },
      { slug: 'vm-list', title: 'Listing VMs', summary: 'List VMs', commands: ['vm'], subcommands: ['ls'] },
      ref.groups[1],
    ],
  };
  const files = renderCliReference(split);
  const first = files.get('cli/vm.mdx')!;
  const second = files.get('cli/vm-list.mdx')!;
  assert.ok(first.includes('<span id="demo-vm"></span>') && first.includes('demo vm [COMMAND]'));
  assert.ok(first.includes('## `demo vm create`') && !first.includes('## `demo vm ls`'));
  assert.ok(second.includes('<span id="demo-vm-ls"></span>') && !second.includes('demo vm create'));
  assert.ok(!second.includes('<span id="demo-vm"></span>'));
  const index = files.get('cli/index.mdx')!;
  assert.ok(index.includes('[`demo vm create`](/demo/reference/cli/vm#demo-vm-create)'));
  assert.ok(index.includes('[`demo vm ls`](/demo/reference/cli/vm-list#demo-vm-ls)'));
  for (const [name, page] of files) if (name.endsWith('.mdx')) assertUniqueAnchors(name, page);
  // Every visible subcommand is placed exactly once.
  const groups = split.groups.slice();
  groups[1] = { ...groups[1], subcommands: [] };
  assert.throws(() => renderCliReference({ ...split, groups }), /Unplaced: ls/);
  groups[1] = { ...groups[1], subcommands: ['ls', 'create'] };
  assert.throws(() => renderCliReference({ ...split, groups }), /duplicated: create/);
});

test('groups must cover every visible command exactly once', () => {
  assert.throws(
    () => renderCliReference({ ...ref, groups: [ref.groups[0]] }),
    /Ungrouped: up/
  );
  assert.throws(
    () => renderCliReference({ ...ref, groups: [...ref.groups, { slug: 'x', title: 'x', summary: 'x', commands: ['up'] }] }),
    /duplicated: up/
  );
});

test('examples split from help text', () => {
  assert.deepEqual(splitExamples('Intro.\n\nExamples:\n  # One\n  a b\n  $ c d  # Two\n\nNotes:\n  more'), {
    prose: 'Intro.\n\nNotes:\n  more',
    examples: [
      { command: 'a b', description: 'One' },
      { command: 'c d', description: 'Two' },
    ],
  });
  assert.deepEqual(splitExamples('No examples'), { prose: 'No examples', examples: [] });
});

test('types read as types', () => {
  assert.equal(displayType({ type: 'NAME' }), 'string');
  assert.equal(displayType({ type: 'Int' }), 'integer');
  assert.equal(displayType({ type: '[String]' }), 'string');
  assert.equal(displayType({ type: 'x', possible_values: ['a', 'b'] }), '`a` \\| `b`');
});

// ---------------------------------------------------------------- MCP

const schema = {
  type: 'object',
  properties: {
    pid: { type: 'integer', description: 'Process' },
    mode: { type: 'string', enum: ['fast', 'slow'], description: 'Mode' },
    target: { type: 'object', properties: { id: { type: 'string', description: 'Id' } }, required: ['id'] },
  },
  required: ['pid', 'mode'],
};

const mcp: McpReference = {
  product: 'demo',
  server: 'demo mcp',
  generator: 'gen',
  source: 'src',
  version: '1.2.3',
  description: 'Demo tools.',
  categories: [
    { slug: 'act', title: 'Action tools', summary: 'Act' },
    { slug: 'look', title: 'Inspection tools', summary: 'Look' },
  ],
  platforms: ['macOS', 'Linux'],
  tools: [
    { name: 'tap', description: 'Tap.', category: 'act', input_schema: schema, annotations: { destructive: true } },
    {
      name: 'peek',
      description: 'Peek.',
      category: 'look',
      input_schema: schema,
      annotations: { read_only: true, idempotent: true },
      platforms: ['macOS'],
      variants: [
        { platform: 'macOS', description: 'Peek via AX.', input_schema: schema },
        {
          platform: 'Linux',
          description: 'Peek via AT-SPI.',
          input_schema: { ...schema, properties: { ...schema.properties, depth: { type: 'integer', description: 'Depth' } } },
        },
      ],
    },
  ],
};

test('MCP reference: index with old anchors, category pages, platform tabs', () => {
  const files = renderMcpReference(mcp);
  assert.deepEqual([...files.keys()], ['mcp-tools/index.mdx', 'mcp-tools/act.mdx', 'mcp-tools/look.mdx', 'mcp-tools/meta.json']);
  const index = files.get('mcp-tools/index.mdx')!;
  assert.ok(index.includes('## Action tools'));
  assert.ok(index.includes('<span id="tap"></span>[`tap`](/demo/reference/mcp-tools/act#tap)'));
  const act = files.get('mcp-tools/act.mdx')!;
  assert.ok(act.includes('## `tap`'));
  assert.ok(act.includes('Effect: destructive.'));
  assert.ok(act.includes('| <span id="tap--pid"></span>`pid` | `integer` | required | Process. |'));
  assert.ok(act.includes('| <span id="tap--target-id"></span>`target.id` | `string` |  | Id. |'));
  assert.ok(act.includes('{"pid":1,"mode":"fast"}'));
  const look = files.get('mcp-tools/look.mdx')!;
  assert.ok(look.startsWith('---') && look.includes("import { Tab, Tabs } from 'fumadocs-ui/components/tabs';"));
  assert.ok(look.includes(`<Tabs items={['macOS', 'Linux']} groupId="platform" persist>`));
  assert.ok(look.includes('Peek via AX.') && look.includes('Peek via AT-SPI.'));
  // Shared parameters once, outside the tabs; the Linux-only one inside.
  assert.ok(look.includes('**Parameters on every platform**'));
  assert.ok(look.includes('<span id="peek--linux--depth"></span>'));
  assert.equal(look.split('<span id="peek--pid"></span>').length, 2);
  for (const [name, page] of files) if (name.endsWith('.mdx')) assertUniqueAnchors(name, page);
});

test('example arguments satisfy the schema', () => {
  assert.deepEqual(exampleArguments('tap', schema), { pid: 1, mode: 'fast' });
  assert.equal(validate({ pid: 'x', mode: 'fast' }, schema), 'pid: expected integer, got string');
  assert.equal(exampleArguments('none', { type: 'object', properties: {} }), null);
  assert.equal(schemaType({ oneOf: [{ type: 'string' }, { type: 'integer' }] }), 'string | integer');
  assert.equal(schemaType({ type: 'object', additionalProperties: { type: 'string' } }), 'map of string');
});

test('single-page mode for small servers', () => {
  const files = renderMcpReference({ ...mcp, singlePage: true, tools: [mcp.tools[0]] });
  assert.deepEqual([...files.keys()], ['mcp-tools.mdx']);
  assert.ok(files.get('mcp-tools.mdx')!.includes('### `tap`'));
});

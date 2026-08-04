import assert from 'node:assert/strict';
import { mkdtemp, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { createCuaMcpServer } from '../dist/server.js';

const skillRoot = resolve(import.meta.dirname, '../../rust/Skills/cua-driver');
class FakeAdapter {
  calls = [];
  async listTools() {
    return [
      {
        name: 'get_screen_size',
        description: 'Real bounded Cua Driver route',
        inputSchema: { type: 'object', additionalProperties: false },
        annotations: {
          readOnlyHint: true,
          destructiveHint: false,
          idempotentHint: true,
          openWorldHint: false,
        },
      },
    ];
  }
  async callTool(name, args) {
    this.calls.push({ name, args });
    return {
      content: [
        { type: 'text', text: '1920x1080' },
        { type: 'image', data: 'aW1hZ2U=', mimeType: 'image/png' },
      ],
      structuredContent: { width: 1920, height: 1080 },
    };
  }
  async close() {}
}

async function connectedPair(root = skillRoot) {
  const adapter = new FakeAdapter();
  const server = await createCuaMcpServer({ adapter, skillRoot: root });
  const client = new Client({ name: 'test-client', version: '1.0.0' }, { capabilities: {} });
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);
  return { adapter, client, server };
}

test('initialize instructions carry a compact bundled skill catalog', async (t) => {
  const { client, server } = await connectedPair();
  t.after(() => Promise.all([client.close(), server.close()]));
  const instructions = client.getInstructions();
  assert.match(instructions, /<available_skills>/);
  assert.match(instructions, /<name>cua-driver<\/name>/);
  assert.ok(instructions.length < 1600);
  assert.doesNotMatch(instructions, /## Workflow/);
});

test('tools/list and tools/call preserve driver and skill routing', async (t) => {
  const { adapter, client, server } = await connectedPair();
  t.after(() => Promise.all([client.close(), server.close()]));
  const listed = await client.listTools();
  assert.deepEqual(
    listed.tools.map((tool) => tool.name),
    ['get_screen_size', 'load-skill']
  );
  assert.equal(listed.tools[1].annotations.readOnlyHint, true);
  const result = await client.callTool({ name: 'get_screen_size', arguments: { session: 'poc' } });
  assert.deepEqual(adapter.calls, [{ name: 'get_screen_size', args: { session: 'poc' } }]);
  assert.equal(result.content[1].type, 'image');
  assert.deepEqual(result.structuredContent, { width: 1920, height: 1080 });
});

test('load-skill returns SKILL.md and blocks path traversal', async (t) => {
  const { client, server } = await connectedPair();
  t.after(() => Promise.all([client.close(), server.close()]));
  const loaded = await client.callTool({ name: 'load-skill', arguments: { name: 'cua-driver' } });
  assert.equal(loaded.isError, undefined);
  assert.match(loaded.content[0].text, /^---\nname: cua-driver/m);
  const blocked = await client.callTool({
    name: 'load-skill',
    arguments: { name: 'cua-driver', path: '../VERSION' },
  });
  assert.equal(blocked.isError, true);
  assert.match(blocked.content[0].text, /traversal/);
});

test('load-skill rejects symlinks that escape the bundled root', async (t) => {
  const temporaryRoot = await mkdtemp(join(tmpdir(), 'cua-skill-test-'));
  t.after(() => rm(temporaryRoot, { recursive: true, force: true }));
  await symlink(resolve(skillRoot, 'SKILL.md'), join(temporaryRoot, 'SKILL.md'));
  await symlink(resolve(skillRoot, '../../VERSION'), join(temporaryRoot, 'escape.md'));
  const { client, server } = await connectedPair(temporaryRoot);
  t.after(() => Promise.all([client.close(), server.close()]));
  const blocked = await client.callTool({
    name: 'load-skill',
    arguments: { name: 'cua-driver', path: 'escape.md' },
  });
  assert.equal(blocked.isError, true);
  assert.match(blocked.content[0].text, /escapes/);
});

test('resources expose and read bundled companion files', async (t) => {
  const { client, server } = await connectedPair();
  t.after(() => Promise.all([client.close(), server.close()]));
  const resources = await client.listResources();
  const browser = resources.resources.find((resource) => resource.uri.endsWith('/BROWSER.md'));
  assert.ok(browser);
  const loaded = await client.readResource({ uri: browser.uri });
  assert.match(loaded.contents[0].text, /browser/i);
});

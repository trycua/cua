import assert from 'node:assert/strict';
import test from 'node:test';
import { mapDaemonInventory, UniffiDaemonAdapter } from '../dist/adapter.js';

test('daemon inventory mapping preserves schemas, annotations, and extensions', () => {
  const [tool] = mapDaemonInventory({
    tools: [
      {
        name: 'click',
        description: 'click',
        input_schema: {
          type: 'object',
          properties: { x: { type: 'integer' } },
          required: ['x'],
        },
        output_schema: { type: 'object' },
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: false,
        capabilities: ['input.pointer.click'],
        risk: { class: 'r2' },
      },
    ],
  });
  assert.equal(tool.inputSchema.required[0], 'x');
  assert.equal(tool.outputSchema.type, 'object');
  assert.equal(tool.annotations.destructiveHint, true);
  assert.deepEqual(tool.capabilities, ['input.pointer.click']);
  assert.deepEqual(tool.risk, { class: 'r2' });
});

test('UniFFI adapter preserves raw MCP content and releases only its client', async () => {
  const calls = [];
  const driver = {
    async listToolsJson() {
      return JSON.stringify({ tools: [] });
    },
    async callTool(name, args) {
      calls.push({ name, args: JSON.parse(args) });
      return {
        rawJson: JSON.stringify({
          content: [
            { type: 'text', text: 'ok' },
            { type: 'image', data: 'AA==', mimeType: 'image/png' },
          ],
          structuredContent: { ok: true },
          isError: false,
        }),
      };
    },
    async metadata() {
      return { embedded: false, pid: 42 };
    },
    async shutdown() {
      calls.push({ shutdown: true });
    },
    uniffiDestroy() {
      calls.push({ destroyed: true });
    },
  };
  const adapter = new UniffiDaemonAdapter(driver);
  const result = await adapter.callTool('get_desktop_state', { session: 'same' });
  assert.equal(result.content[1].type, 'image');
  assert.deepEqual(result.structuredContent, { ok: true });
  await adapter.close();
  assert.deepEqual(calls, [
    { name: 'get_desktop_state', args: { session: 'same' } },
    { shutdown: true },
    { destroyed: true },
  ]);
});

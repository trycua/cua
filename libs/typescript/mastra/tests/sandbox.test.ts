import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Agent } from '@mastra/core/agent';
import { Workspace, createWorkspaceTools } from '@mastra/core/workspace';
import { PNG } from 'pngjs';
import { CuaFleetSandbox } from '../src/index.js';
import { decodeGuestResponse, type FleetSession } from '../src/fleet-session.js';

const png = PNG.sync.write(new PNG({ width: 1, height: 1 }));
const options = {
  id: 'test',
  poolName: 'test-pool',
  clientId: 'private-client',
  clientSecret: 'private-secret',
};
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
class Session implements FleetSession {
  starts = 0;
  closes = 0;
  calls: { name: string; params: Record<string, unknown> }[] = [];
  startImpl = async () => {};
  closeImpl = async () => {};
  commandImpl = async (name: string): Promise<Record<string, unknown>> => {
    if (name === 'screenshot') return { image_data: png.toString('base64') };
    if (name === 'get_screen_size') return { size: { width: 1280, height: 720 } };
    if (name === 'get_cursor_position') return { position: { x: 10, y: 20 } };
    return { success: true };
  };
  async start() {
    this.starts++;
    await this.startImpl();
  }
  async close() {
    this.closes++;
    await this.closeImpl();
  }
  async command(name: string, params: Record<string, unknown>) {
    this.calls.push({ name, params });
    return this.commandImpl(name);
  }
}
class Sandbox extends CuaFleetSandbox {
  sessions: Session[];
  allocations = 0;
  constructor(...sessions: Session[]) {
    super(options);
    this.sessions = sessions;
  }
  protected override createSession() {
    return this.sessions[this.allocations++]!;
  }
}

test('computer surface maps commands and normalizes images, dimensions, and keys', async () => {
  const session = new Session();
  const sandbox = new Sandbox(session);
  const c = sandbox.computer;
  const image = await c.screenshot();
  assert.equal(image.mediaType, 'image/png');
  assert.deepEqual(image.data, new Uint8Array(png));
  await c.leftClick(1, 2);
  await c.rightClick(3, 4);
  await c.doubleClick(5, 6);
  await c.moveMouse(7, 8);
  await c.drag({ x: 9, y: 10 }, { x: 11, y: 12 });
  await c.scroll('up', 2);
  await c.scroll('down', 3);
  await c.type('hello\nworld');
  await c.press('Enter');
  await c.press(['Control', 'Shift', 's']);
  await c.press('ArrowUp');
  await c.press('F12');
  await c.press('Escape');
  assert.deepEqual(await c.getScreenSize(), { width: 1280, height: 720 });
  assert.deepEqual(await c.getCursorPosition(), { x: 10, y: 20 });
  assert.deepEqual(session.calls, [
    { name: 'screenshot', params: { format: 'png' } },
    { name: 'left_click', params: { x: 1, y: 2 } },
    { name: 'right_click', params: { x: 3, y: 4 } },
    { name: 'double_click', params: { x: 5, y: 6 } },
    { name: 'move_cursor', params: { x: 7, y: 8 } },
    {
      name: 'drag',
      params: {
        path: [
          [9, 10],
          [11, 12],
        ],
        button: 'left',
      },
    },
    { name: 'scroll_up', params: { clicks: 2 } },
    { name: 'scroll_down', params: { clicks: 3 } },
    { name: 'type_text', params: { text: 'hello\nworld' } },
    { name: 'press_key', params: { key: 'enter' } },
    { name: 'hotkey', params: { keys: ['ctrl', 'shift', 's'] } },
    { name: 'press_key', params: { key: 'up' } },
    { name: 'press_key', params: { key: 'f12' } },
    { name: 'press_key', params: { key: 'esc' } },
    { name: 'get_screen_size', params: {} },
    { name: 'get_cursor_position', params: {} },
  ]);
  assert.equal(session.starts, 1);
  await sandbox.destroy();
});

test('invalid arguments fail before allocating or sending a command', async () => {
  const session = new Session();
  const sandbox = new Sandbox(session);
  const c = sandbox.computer;
  for (const action of [
    () => c.leftClick(-1, 2),
    () => c.moveMouse(NaN, 2),
    () => c.scroll('up', 0),
    () => c.scroll('down', 1.5),
    () => c.press([]),
    () => c.press(''),
    () => c.press('not-a-key'),
    () => c.drag({ x: 0, y: 0 }, { x: Infinity, y: 2 }),
  ]) {
    await assert.rejects(async () => action(), /INVALID_|UNSUPPORTED_KEY/);
  }
  assert.equal(session.starts, 0);
  assert.equal(session.calls.length, 0);
});

test('malformed guest results are rejected', async () => {
  const session = new Session();
  const sandbox = new Sandbox(session);
  const truncated = Buffer.alloc(24);
  Buffer.from('89504e470d0a1a0a', 'hex').copy(truncated);
  truncated.writeUInt32BE(1, 16);
  truncated.writeUInt32BE(1, 20);
  const invalidCrc = Buffer.from(png);
  invalidCrc[29] = invalidCrc[29]! ^ 1;
  for (const response of [
    {},
    { image_data: 'not an image' },
    { image_data: Buffer.alloc(24).toString('base64') },
    { image_data: truncated.toString('base64') },
    { image_data: invalidCrc.toString('base64') },
  ]) {
    session.commandImpl = async () => response;
    await assert.rejects(sandbox.computer.screenshot(), /INVALID_SCREENSHOT/);
  }
  session.commandImpl = async () => ({
    size: { width: 0, height: 720 },
    position: { x: '1', y: 2 },
  });
  await assert.rejects(sandbox.computer.getScreenSize(), /INVALID_ARGUMENT/);
  await assert.rejects(sandbox.computer.getCursorPosition(), /INVALID_ARGUMENT/);
  await sandbox.destroy();
});

test('concurrent start coalesces and destroy waits for acquisition', async () => {
  const gate = deferred();
  const session = new Session();
  session.startImpl = () => gate.promise;
  const sandbox = new Sandbox(session);
  const first = sandbox.start();
  const second = sandbox.start();
  assert.equal(first, second);
  assert.equal(session.starts, 1);
  const destroy = sandbox.destroy();
  assert.equal(session.closes, 0);
  await assert.rejects(sandbox.start(), /DESTROYED/);
  gate.resolve();
  await first;
  await destroy;
  assert.equal(session.closes, 1);
  assert.equal(sandbox.status, 'destroyed');
  await assert.rejects(sandbox.computer.screenshot(), /DESTROYED/);
  await sandbox.destroy();
  assert.equal(session.closes, 1);
});

test('destroy drains an active command and rejects queued or new operations', async () => {
  const entered = deferred();
  const gate = deferred();
  const session = new Session();
  session.commandImpl = async () => {
    entered.resolve();
    await gate.promise;
    return {};
  };
  const sandbox = new Sandbox(session);
  const active = sandbox.computer.type('first');
  await entered.promise;
  const queued = sandbox.computer.type('second');
  const queuedRejected = assert.rejects(queued, /DESTROYED/);
  const destroy = sandbox.destroy();
  assert.equal(session.closes, 0);
  await assert.rejects(sandbox.computer.type('third'), /DESTROYED/);
  gate.resolve();
  await active;
  await queuedRejected;
  await destroy;
  assert.equal(session.calls.length, 1);
  assert.equal(session.closes, 1);
});

test('failed startup releases its session and permits a clean retry', async () => {
  const failed = new Session();
  failed.startImpl = async () => {
    throw new Error(options.clientSecret);
  };
  const next = new Session();
  const sandbox = new Sandbox(failed, next);
  await assert.rejects(sandbox.start(), { message: 'Cua Fleet: START_FAILED' });
  assert.equal(failed.closes, 1);
  assert.equal(sandbox.status, 'error');
  assert.deepEqual(await sandbox.start(), { outcome: 'created' });
  assert.deepEqual(await sandbox.start(), { outcome: 'connected' });
  assert.equal(next.starts, 1);
  await sandbox.destroy();
});

test('failed cleanup remains retryable and blocks further use', async () => {
  const session = new Session();
  let fail = true;
  session.closeImpl = async () => {
    if (fail) throw new Error(options.clientSecret);
  };
  const sandbox = new Sandbox(session);
  await sandbox.start();
  await assert.rejects(sandbox.destroy(), { message: 'Cua Fleet: CLEANUP_PENDING' });
  assert.equal(sandbox.status, 'error');
  await assert.rejects(sandbox.start(), /DESTROYED/);
  fail = false;
  await sandbox.destroy();
  assert.equal(session.closes, 2);
  assert.equal(sandbox.status, 'destroyed');
});

test('startup cleanup failure retains session for explicit destruction', async () => {
  const session = new Session();
  let fail = true;
  session.startImpl = async () => {
    throw new Error('failed');
  };
  session.closeImpl = async () => {
    if (fail) throw new Error('release failed');
  };
  const sandbox = new Sandbox(session);
  await assert.rejects(sandbox.start(), /START_FAILED_CLEANUP_PENDING/);
  fail = false;
  await sandbox.destroy();
  assert.equal(session.closes, 2);
});

test('stop explicitly rejects pause and configuration and raw errors remain private', async () => {
  const session = new Session();
  const sandbox = new Sandbox(session);
  await sandbox.start();
  await assert.rejects(sandbox.stop(), /PAUSE_UNSUPPORTED_USE_DESTROY/);
  assert.equal(sandbox.status, 'running');
  assert.equal(session.closes, 0);
  session.commandImpl = async () => {
    throw new Error(`Authorization: ${options.clientSecret}`);
  };
  await assert.rejects(sandbox.computer.type('hello'), { message: 'Cua Fleet: COMMAND_FAILED' });
  const serialized = JSON.stringify(new CuaFleetSandbox(options));
  assert.ok(!serialized.includes(options.clientSecret));
  assert.ok(!serialized.includes(options.clientId));
  assert.equal(sandbox.supportsCheckpoints, false);
  await sandbox.snapshot();
  await sandbox.destroy();
});

test('Mastra discovers computer tools and Agent.generate executes a model tool call offline', async () => {
  const session = new Session();
  const sandbox = new Sandbox(session);
  const workspace = new Workspace({ sandbox });
  const tools = await createWorkspaceTools(workspace);
  assert.equal(
    Object.keys(tools).filter((name) => name.startsWith('mastra_workspace_computer_')).length,
    11
  );
  assert.ok(!('mastra_workspace_execute_command' in tools));
  const usage = { inputTokens: 1, outputTokens: 1, totalTokens: 2 };
  const calls: unknown[] = [];
  const responses = [
    {
      content: [
        {
          type: 'tool-call' as const,
          toolCallId: 'screen-1',
          toolName: 'mastra_workspace_computer_screenshot',
          input: '{}',
        },
      ],
      finishReason: 'tool-calls' as const,
      usage,
      warnings: [],
    },
    {
      content: [{ type: 'text' as const, text: 'Desktop screenshot received.' }],
      finishReason: 'stop' as const,
      usage,
      warnings: [],
    },
  ];
  const model = {
    specificationVersion: 'v2' as const,
    provider: 'fixture',
    modelId: 'scripted',
    supportedUrls: {},
    async doGenerate(input: unknown) {
      calls.push(input);
      const response = responses[calls.length - 1];
      assert.ok(response, 'unexpected extra model call');
      return response;
    },
    async doStream(): Promise<never> {
      throw new Error('Streaming not used');
    },
  };
  const agent = new Agent({
    id: 'offline-desktop',
    name: 'Offline Desktop',
    instructions: 'Inspect the desktop.',
    model,
    workspace,
  });
  const result = await agent.generate('Inspect the desktop.', { maxSteps: 3 });
  assert.equal(result.text, 'Desktop screenshot received.');
  assert.deepEqual(session.calls, [{ name: 'screenshot', params: { format: 'png' } }]);
  assert.equal(calls.length, 2);
  const prompt = calls[1] as { prompt: { content: unknown }[] };
  const modelInput = JSON.stringify(prompt.prompt);
  assert.ok(modelInput.includes('tool-result'));
  assert.ok(modelInput.includes('image/png'), 'the model receives a successful image result');
  assert.ok(modelInput.includes(png.toString('base64')), 'the model receives the screenshot bytes');
  assert.ok(!modelInput.includes('INVALID_SCREENSHOT'));
  await workspace.destroy();
  assert.equal(session.closes, 1);
});

test('guest decoder accepts JSON and SSE JSON but rejects malformed or scalar responses', () => {
  const bytes = (text: string) => new TextEncoder().encode(text).buffer;
  assert.deepEqual(decodeGuestResponse(bytes('{"success":true}')), { success: true });
  assert.deepEqual(decodeGuestResponse(bytes('data: {"success":true}\n\n')), { success: true });
  for (const value of ['invalid private-secret', 'null', '[]', '42', 'data: invalid']) {
    assert.throws(() => decodeGuestResponse(bytes(value)), {
      message: 'Cua Fleet: INVALID_GUEST_RESPONSE',
    });
  }
});

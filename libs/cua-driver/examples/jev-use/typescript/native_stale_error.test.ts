// Mirrors python/tests/test_native_stale_error.py. No native Driver is started.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { nativeTask } from './native_tasks.js';
import { DriverToolError } from './run.js';
import { isStaleTokenError, runTask } from './run_native.js';

const STALE_MESSAGE = 'click failed: element_token is stale; get_window_state again';

test('structured code precedes diagnostic text', () => {
  for (const code of ['permission_denied', 'tool_invocation_failed', 'future_error', ' ']) {
    assert.equal(isStaleTokenError(new DriverToolError(STALE_MESSAGE, code)), false, code);
  }
});

test('structured stale code does not need diagnostic text', () => {
  assert.equal(isStaleTokenError(new DriverToolError('refused', 'stale_element_token')), true);
});

test('code-less legacy messages remain supported', () => {
  for (const error of [
    new DriverToolError(STALE_MESSAGE),
    new DriverToolError(STALE_MESSAGE, ''),
    new Error(STALE_MESSAGE),
  ]) {
    assert.equal(isStaleTokenError(error), true);
  }
  assert.equal(isStaleTokenError(new Error('transport failed')), false);
});

test('wrapped error does not borrow its causes stale code', () => {
  const error = new Error('transport failed', {
    cause: new DriverToolError('refused', 'stale_element_token'),
  });
  assert.equal(isStaleTokenError(error), false);
});

test('native loop preserves code precedence and legacy stale recovery', async (context) => {
  for (const code of [
    'permission_denied',
    'tool_invocation_failed',
    'future_error',
    'stale_element_token',
    undefined,
  ]) {
    await context.test(code ?? 'legacy no code', async (t) => {
      const directory = await mkdtemp(join(tmpdir(), 'jev-native-stale-'));
      try {
        const payload = JSON.parse(
          readFileSync(
            new URL('../fixtures/native/gtk3-window-state-initial-v1.json', import.meta.url),
            'utf8'
          )
        );
        const statePath = join(directory, 'state.json');
        const logPath = join(directory, 'run.jsonl');
        const task = nativeTask('gtk3-counter', statePath, { pid: payload.pid });
        const state = { schema: task.oracle.schema, pid: payload.pid, counter: 0 };
        await writeFile(statePath, JSON.stringify(state));
        const actions: Record<string, any>[] = [];
        const reads: Record<string, any>[] = [];
        const isStale = code === undefined || code === 'stale_element_token';
        t.mock.method(Client.prototype, 'connect', async () => {});
        t.mock.method(Client.prototype, 'close', async () => {});
        t.mock.method(Client.prototype, 'listTools', async () => ({ tools: [] }));
        t.mock.method(console, 'log', () => {});
        t.mock.method(
          Client.prototype,
          'callTool',
          async (request: { name: string; arguments?: Record<string, any> }) => {
            const args = request.arguments ?? {};
            let data: Record<string, any>;
            if (request.name === 'list_windows') {
              data = { windows: [{ window_id: payload.window_id, title: task.scope.windowTitle }] };
            } else if (request.name === 'get_window_state') {
              reads.push({ ...args });
              data = structuredClone(payload);
              const snapshot = `s${String(reads.length).padStart(8, '0')}`;
              data.snapshot_id = snapshot;
              for (const element of data.elements)
                element.element_token = `${snapshot}:${element.element_index}`;
            } else if (request.name === 'click') {
              actions.push({ ...args });
              if (actions.length === 1 || !isStale) {
                return {
                  isError: true,
                  structuredContent: code === undefined ? {} : { code },
                  content: [{ type: 'text', text: STALE_MESSAGE }],
                };
              }
              state.counter = 3;
              await writeFile(statePath, JSON.stringify(state));
              data = { effect: 'confirmed' };
            } else {
              throw new Error(`unexpected tool ${request.name}`);
            }
            return { isError: false, structuredContent: data, content: [] };
          }
        );
        const outcome = await runTask(
          {
            task: task.id,
            provider: 'mock',
            pid: payload.pid,
            stateFile: statePath,
            allowForeground: false,
            platform: 'linux',
            log: logPath,
          },
          task
        );
        const events = (await readFile(logPath, 'utf8'))
          .trim()
          .split('\n')
          .map((line) => JSON.parse(line));
        assert.equal(outcome, isStale ? 'verified' : 'unknown');
        assert.equal(actions.length, isStale ? 2 : 1);
        assert.equal(reads.length, isStale ? 2 : 1);
        assert.equal(JSON.parse(await readFile(statePath, 'utf8')).counter, isStale ? 3 : 0);
        assert.equal(
          events.filter((event) => event.action_error === 'stale_element_token').length,
          isStale ? 1 : 0
        );
        if (isStale) {
          assert.notEqual(actions[0].element_token, actions[1].element_token);
        } else {
          assert.equal(events.at(-1).phase, 'action');
        }
      } finally {
        await rm(directory, { recursive: true, force: true });
      }
    });
  }
});

import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import type { Candidate } from './core.js';
import {
  appkitTask,
  checkIntermediateEffect,
  COUNTER_TARGET,
  NativeTask,
  OracleError,
} from './native_tasks.js';
import { Driver } from './run.js';
import { parseArgs, pollOracle, runTask } from './run_native.js';

test('checkIntermediateEffect for counter', () => {
  const task = appkitTask('appkit-counter', '/tmp/none.json', { pid: 42 });
  // Expected exact +1 increment
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0 }, { counter: 1 }), true);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 1 }, { counter: 2 }), true);
  // Stale counter (not yet incremented)
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0 }, { counter: 0 }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 1 }, { counter: 1 }), false);
  // Missing or invalid pre/post counter
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', null, { counter: 1 }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 'bad' }, { counter: 1 }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0 }, { counter: 'bad' }), false);
});

test('checkIntermediateEffect counter rejects boolean, fractional, and non-finite values', () => {
  const task = appkitTask('appkit-counter', '/tmp/none.json', { pid: 42 });
  // Boolean values must be rejected
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: false }, { counter: true }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0 }, { counter: true }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: false }, { counter: 1 }), false);
  // Fractional numbers must be rejected
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0 }, { counter: 1.5 }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0.5 }, { counter: 1.5 }), false);
  // Non-finite values must be rejected
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0 }, { counter: Number.NaN }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', { counter: 0 }, { counter: Number.POSITIVE_INFINITY }), false);
});

test('checkIntermediateEffect for choose-size', () => {
  const task = appkitTask('appkit-choose-size', '/tmp/none.json', { pid: 42 });
  // Radio Large selected
  assert.equal(checkIntermediateEffect(task, 'ax:radio:large', null, { size: 'large' }), true);
  // Radio Large not yet selected
  assert.equal(checkIntermediateEffect(task, 'ax:radio:large', null, { size: '' }), false);
  assert.equal(checkIntermediateEffect(task, 'ax:radio:large', null, { size: 'small' }), false);
});

test('checkIntermediateEffect for unsupported and null candidates', () => {
  const task = appkitTask('appkit-save-note', '/tmp/none.json', { pid: 42 });
  // Text entry has no intermediate oracle field -> null (conservative full polling)
  assert.equal(checkIntermediateEffect(task, 'ax:text_input:note:set:note', {}, {}), null);
  assert.equal(checkIntermediateEffect(task, null, {}, {}), null);
  assert.equal(checkIntermediateEffect(task, 'ax:unknown', {}, {}), null);
});

test('custom task with familiar candidate IDs retains baseline polling', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, counter: 1 }),
      'utf8'
    );
    const baseTask = appkitTask('appkit-counter', statePath, { pid: 42 });
    const customTask = new NativeTask({
      id: 'custom-counter',
      goal: 'Custom task using familiar IDs',
      scope: baseTask.scope,
      allowedActions: new Set(['press']),
      oracle: baseTask.oracle,
      check: () => 'pending',
      steps: [{ description: 'Increment', candidateId: 'ax:button:increment', times: 3 }],
      // Explicitly no intermediateEffect
    });

    assert.equal(customTask.intermediateEffect, null);
    assert.equal(checkIntermediateEffect(customTask, 'ax:button:increment', { counter: 0 }, { counter: 1 }), null);

    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [customTask.historyEntry(1, candidate.id)];

    let reads = 0;
    customTask.readOracle = async () => {
      reads += 1;
      return { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 1 };
    };

    const result = await pollOracle(customTask, 1, candidate, { counter: 0 }, history);
    assert.equal(result.outcome, 'unknown');
    assert.equal(result.status, 'continuation');
    assert.equal(result.canContinue, true);
    assert.equal(reads, 20);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('pollOracle intermediate immediate effect short-circuits', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, counter: 1 }),
      'utf8'
    );
    const task = appkitTask('appkit-counter', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [task.historyEntry(1, candidate.id)];

    let reads = 0;
    const origRead = task.readOracle.bind(task);
    task.readOracle = async () => {
      reads += 1;
      return origRead();
    };

    const result = await pollOracle(task, 1, candidate, { counter: 0 }, history);
    assert.equal(result.outcome, 'unknown');
    assert.equal(result.status, 'intermediate_witnessed');
    assert.equal(result.canContinue, true);
    assert.equal(reads, 1);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('pollOracle delayed intermediate effect waits until witnessed', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, counter: 0 }),
      'utf8'
    );
    const task = appkitTask('appkit-counter', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [task.historyEntry(1, candidate.id)];

    let reads = 0;
    task.readOracle = async () => {
      reads += 1;
      if (reads < 3) {
        return { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 0 };
      }
      return { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 1 };
    };

    const result = await pollOracle(task, 1, candidate, { counter: 0 }, history);
    assert.equal(result.status, 'intermediate_witnessed');
    assert.equal(result.canContinue, true);
    assert.equal(reads, 3);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('pollOracle intermediate effect never applied times out conservatively', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, counter: 0 }),
      'utf8'
    );
    const task = appkitTask('appkit-counter', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [task.historyEntry(1, candidate.id)];

    let reads = 0;
    task.readOracle = async () => {
      reads += 1;
      return { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 0 };
    };

    const result = await pollOracle(task, 1, candidate, { counter: 0 }, history);
    assert.equal(result.outcome, 'unknown');
    assert.equal(result.status, 'intermediate_timeout');
    // Must NOT continue to dependent mutation!
    assert.equal(result.canContinue, false);
    assert.equal(reads, 20);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('terminal action with satisfied low-level effect waits for terminal oracle', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    const task = appkitTask('appkit-counter', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    // 3 increments performed (no declared steps remain)
    const history = [
      task.historyEntry(1, candidate.id),
      task.historyEntry(2, candidate.id),
      task.historyEntry(3, candidate.id),
    ];

    let reads = 0;
    task.readOracle = async () => {
      reads += 1;
      if (reads < 3) {
        return { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 2 };
      }
      return { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 3 };
    };

    const result = await pollOracle(task, 3, candidate, { counter: 2 }, history);
    assert.equal(result.outcome, 'verified');
    assert.equal(result.status, 'verified');
    assert.equal(result.canContinue, false);
    assert.equal(reads, 3);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('no-steps fallback retains baseline polling', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, counter: 1 }),
      'utf8'
    );
    const baseTask = appkitTask('appkit-counter', statePath, { pid: 42 });
    const noStepsTask = new NativeTask({
      id: 'appkit-counter-no-steps',
      goal: baseTask.goal,
      scope: baseTask.scope,
      allowedActions: baseTask.allowedActions,
      oracle: baseTask.oracle,
      check: baseTask.check,
      steps: [],
      intermediateEffect: baseTask.intermediateEffect,
    });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [noStepsTask.historyEntry(1, candidate.id)];

    let reads = 0;
    noStepsTask.readOracle = async () => {
      reads += 1;
      return { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 1 };
    };

    const result = await pollOracle(noStepsTask, 1, candidate, { counter: 0 }, history);
    assert.equal(result.outcome, 'unknown');
    assert.equal(result.status, 'continuation');
    assert.equal(result.canContinue, true);
    assert.equal(reads, 20);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('pollOracle refuted state dominance over intermediate increment', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, counter: COUNTER_TARGET + 1 }),
      'utf8'
    );
    const task = appkitTask('appkit-counter', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [task.historyEntry(1, candidate.id)];

    const result = await pollOracle(task, 1, candidate, { counter: COUNTER_TARGET }, history);
    assert.equal(result.outcome, 'refuted');
    assert.equal(result.status, 'refuted');
    assert.equal(result.canContinue, false);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('pollOracle early terminal success on step 1', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, counter: COUNTER_TARGET }),
      'utf8'
    );
    const task = appkitTask('appkit-counter', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [task.historyEntry(1, candidate.id)];

    const result = await pollOracle(task, 1, candidate, { counter: 0 }, history);
    assert.equal(result.outcome, 'verified');
    assert.equal(result.status, 'verified');
    assert.equal(result.canContinue, false);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('pollOracle foreign PID and schema failures propagate', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 999, counter: 1 }),
      'utf8'
    );
    const task = appkitTask('appkit-counter', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:button:increment', source: 'ax', tool: 'press', description: 'test', arguments: { pid: 42 } };
    const history = [task.historyEntry(1, candidate.id)];

    await assert.rejects(pollOracle(task, 1, candidate, { counter: 0 }, history), OracleError);

    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.foreign_schema_v1', pid: 42, counter: 1 }),
      'utf8'
    );
    await assert.rejects(pollOracle(task, 1, candidate, { counter: 0 }, history), OracleError);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('pollOracle unsupported save-note text entry retains full polling', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-test-'));
  const statePath = join(dir, 'state.json');
  try {
    await writeFile(
      statePath,
      JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid: 42, note_saved: null }),
      'utf8'
    );
    const task = appkitTask('appkit-save-note', statePath, { pid: 42 });
    const candidate: Candidate = { id: 'ax:text_input:note:set:note', source: 'ax', tool: 'set_text', description: 'test', arguments: { pid: 42 } };
    const history = [task.historyEntry(1, candidate.id)];

    let reads = 0;
    task.readOracle = async () => {
      reads += 1;
      return { schema: 'cua.appkit_task_state_v1', pid: 42, note_saved: null };
    };

    const result = await pollOracle(task, 1, candidate, {}, history);
    assert.equal(result.outcome, 'unknown');
    assert.equal(result.status, 'continuation');
    assert.equal(result.canContinue, true);
    assert.equal(reads, 20);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('discriminator fails stale counter mutant and dispatch-only mutant', () => {
  const task = appkitTask('appkit-counter', '/tmp/none.json', { pid: 42 });
  const preOracle = { counter: 1 };
  const currentOracle = { counter: 1 };

  // Production check rejects stale counter
  assert.equal(checkIntermediateEffect(task, 'ax:button:increment', preOracle, currentOracle), false);

  // Stale counter mutant checks counter > 0
  const staleMutant = (_task: NativeTask, cand: string, _pre: any, curr: any) =>
    cand === 'ax:button:increment' && curr?.counter > 0;
  assert.equal(staleMutant(task, 'ax:button:increment', preOracle, currentOracle), true);
});

test('runTask public boundary integration succeeds', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-int-test-'));
  const statePath = join(dir, 'state.json');
  const logPath = join(dir, 'run.log');
  const initialWs = JSON.parse(
    await readFile(join(__dirname, '../fixtures/native/appkit-window-state-initial-v1.json'), 'utf8')
  );
  const pid = initialWs.pid;

  await writeFile(
    statePath,
    JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid, counter: 0 }),
    'utf8'
  );
  const task = appkitTask('appkit-counter', statePath, { pid });
  const args = parseArgs(['--task', 'appkit-counter', '--pid', String(pid), '--state-file', statePath, '--log', logPath]);

  // Mock external transport and Driver.call
  const origConnect = Client.prototype.connect;
  const origClose = Client.prototype.close;
  const origListTools = Client.prototype.listTools;
  const origCall = Driver.prototype.call;

  let currentCounter = 0;

  try {
    Client.prototype.connect = async function () {};
    Client.prototype.close = async function () {};
    Client.prototype.listTools = async function () {
      return { tools: [{ name: 'click', inputSchema: {} as any }, { name: 'get_window_state', inputSchema: {} as any }] };
    };
    Driver.prototype.call = async function (name: string, callArgs: Record<string, unknown>) {
      if (name === 'list_windows') {
        return { windows: [{ title: task.scope.windowTitle, window_id: initialWs.window_id, is_on_screen: true }] };
      }
      if (name === 'get_window_state') {
        return initialWs;
      }
      if (name === 'press' || name === 'click') {
        currentCounter += 1;
        await writeFile(
          statePath,
          JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid, counter: currentCounter }),
          'utf8'
        );
        return { success: true };
      }
      return {};
    };

    const outcome = await runTask(args, task);
    assert.equal(outcome, 'verified');
    assert.equal(currentCounter, 3);
  } finally {
    Client.prototype.connect = origConnect;
    Client.prototype.close = origClose;
    Client.prototype.listTools = origListTools;
    Driver.prototype.call = origCall;
    await rm(dir, { recursive: true, force: true });
  }
});

test('runTask public boundary stops safely on intermediate timeout without second mutation', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'jev-int-test-'));
  const statePath = join(dir, 'state.json');
  const logPath = join(dir, 'run.log');
  const initialWs = JSON.parse(
    await readFile(join(__dirname, '../fixtures/native/appkit-window-state-initial-v1.json'), 'utf8')
  );
  const pid = initialWs.pid;

  await writeFile(
    statePath,
    JSON.stringify({ schema: 'cua.appkit_task_state_v1', pid, counter: 0 }),
    'utf8'
  );
  const task = appkitTask('appkit-counter', statePath, { pid });
  const args = parseArgs(['--task', 'appkit-counter', '--pid', String(pid), '--state-file', statePath, '--log', logPath]);

  const origConnect = Client.prototype.connect;
  const origClose = Client.prototype.close;
  const origListTools = Client.prototype.listTools;
  const origCall = Driver.prototype.call;

  let dispatchedMutations = 0;

  try {
    Client.prototype.connect = async function () {};
    Client.prototype.close = async function () {};
    Client.prototype.listTools = async function () {
      return { tools: [{ name: 'click', inputSchema: {} as any }, { name: 'get_window_state', inputSchema: {} as any }] };
    };
    Driver.prototype.call = async function (name: string, callArgs: Record<string, unknown>) {
      if (name === 'list_windows') {
        return { windows: [{ title: task.scope.windowTitle, window_id: initialWs.window_id, is_on_screen: true }] };
      }
      if (name === 'get_window_state') {
        return initialWs;
      }
      if (name === 'press' || name === 'click') {
        dispatchedMutations += 1;
        // Never updates counter -> supported intermediate effect never lands!
        return { success: true };
      }
      return {};
    };

    const outcome = await runTask(args, task);
    assert.equal(outcome, 'unknown');
    // Crucial: exactly 1 mutation dispatched; stopped before dispatching step 2!
    assert.equal(dispatchedMutations, 1);
  } finally {
    Client.prototype.connect = origConnect;
    Client.prototype.close = origClose;
    Client.prototype.listTools = origListTools;
    Driver.prototype.call = origCall;
    await rm(dir, { recursive: true, force: true });
  }
});

test('budget exhausted retains terminal polling despite witnessed effect', async () => {
  const base = appkitTask('appkit-counter', '/unused', { pid: 42 });
  const task = new NativeTask({
    id: base.id, goal: base.goal, scope: base.scope, allowedActions: base.allowedActions,
    oracle: base.oracle, check: base.check, steps: base.steps,
    intermediateEffect: base.intermediateEffect, maxSteps: 1,
  });
  let reads = 0;
  task.readOracle = async () => {
    reads += 1;
    return { counter: 1 };
  };
  const candidate: Candidate = {
    id: 'ax:button:increment', source: 'ax', tool: 'click', description: 'increment', arguments: { pid: 42 },
  };
  const result = await pollOracle(task, 1, candidate, { counter: 0 }, [task.historyEntry(1, candidate.id)]);
  assert.equal(result.outcome, 'budget_exhausted');
  assert.equal(reads, 20);
});

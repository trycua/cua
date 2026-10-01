// Task progress in cua.jev_choice_request_v2 (#4313). Mirrors
// python/tests/test_native_progress.py: a native task declares the steps it
// requires, the request reports how often this run has performed each step
// (counted only from the runner's own successful actions), and a step's
// candidate names any earlier step that is not done yet.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { providerObservation, validateRequest } from './choose_action.js';
import { parseWindowState } from './native.js';
import {
  CANVAS_TASK_ID,
  HARNESSES,
  NativeTask,
  nativeChoiceRequest,
  nativeTask,
  performedCounts,
} from './native_tasks.js';
import { NativeAccessibilitySource } from './sources.js';
import type { HistoryEntry, TaskSources } from './tasks.js';

const fixture = (path: string): any => JSON.parse(readFileSync(new URL(`../fixtures/${path}`, import.meta.url), 'utf8'));
const v2Request = () => fixture('jev-choice-request-v2.json');
const v1Request = () => fixture('jev-choice-request-v1.json');

function sources(task: NativeTask, harness: string): TaskSources {
  const payload = fixture(`native/${harness}-window-state-initial-v1.json`);
  return {
    ax: NativeAccessibilitySource.fromObservation(
      parseWindowState(payload, payload.pid, payload.window_id),
      HARNESSES[harness].platform,
      { redact: task.redactText, textMethod: task.textMethod }
    ),
    visualPath: false,
    foregroundIds: new Set(),
  };
}

function requestFor(taskId: string, history: HistoryEntry[], noteText = 'jev-use native note') {
  const task = nativeTask(taskId, '/tmp/none.json', { noteText });
  const taskSources = sources(task, taskId.split('-')[0]);
  return nativeChoiceRequest(task, taskSources, task.plan(taskSources), history) as any;
}

const performed = (task: NativeTask, step: number, id: string) => task.historyEntry(step, id, undefined, { outcome: 'done' });
const byId = (request: any) => Object.fromEntries(request.candidates.map((c: any) => [c.id, c]));

test('v2 accepts progress and the observation carries it', () => {
  const request = v2Request();
  request.progress = [{ step: 'Press "Increment"', done: 2, required: 3 }];
  const validated = validateRequest(request);
  assert.deepEqual(validated.progress, request.progress);
  assert.deepEqual(providerObservation(validated).progress, request.progress);
});

test('v2 without progress keeps its observation, and v1 rejects progress', () => {
  const validated = validateRequest(v2Request());
  assert.deepEqual(validated.progress, []);
  assert.equal('progress' in providerObservation(validated), false);
  const v1 = v1Request();
  v1.progress = [];
  assert.throws(() => validateRequest(v1));
});

test('v2 rejects malformed progress', () => {
  const item = { step: 'Press', done: 0, required: 1 };
  const mutations: Record<string, unknown> = {
    'not a list': { step: 'Press' },
    'too many': Array(17).fill(item),
    'extra key': [{ ...item, value: 'secret' }],
    'missing key': [{ step: 'Press', done: 0 }],
    'empty step': [{ ...item, step: ' ' }],
    'long step': [{ ...item, step: 'x'.repeat(201) }],
    'negative done': [{ ...item, done: -1 }],
    'boolean done': [{ ...item, done: true }],
    'fractional done': [{ ...item, done: 0.5 }],
    'zero required': [{ ...item, required: 0 }],
    'huge required': [{ ...item, required: 65 }],
  };
  for (const [name, progress] of Object.entries(mutations)) {
    const request = v2Request();
    request.progress = progress;
    assert.throws(() => validateRequest(request), Error, name);
  }
});

test('only performed actions count', () => {
  const task = nativeTask('appkit-counter', '/tmp/none.json');
  const history = [
    performed(task, 1, 'ax:button:increment'),
    task.historyEntry(2, 'ax:button:increment', 'background_unsupported'),
    task.historyEntry(3, 'ax:button:increment', undefined, { stale: true }),
    task.historyEntry(4, 'reobserve'),
    performed(task, 5, 'ax:button:increment:foreground'),
  ];
  assert.deepEqual(performedCounts(history), { 'ax:button:increment': 2 });
  assert.deepEqual(task.progress(history), [
    { step: 'Press the button labeled "Increment"', done: 2, required: 3 },
  ]);
  const request = requestFor('appkit-counter', history);
  assert.deepEqual(Object.keys(request.history[0]).sort(), ['outcome', 'selected_id']);
});

test('save is described as waiting for the note until the note is set', () => {
  for (const harness of Object.keys(HARNESSES)) {
    const request = requestFor(`${harness}-save-note`, [], 'secret note');
    assert.match(
      byId(request)['ax:button:save-note'].description,
      /The task requires this only after: Set the text field "Note" to the task parameter "note" \(not done yet\)\.$/
    );
    assert.equal(JSON.stringify(request).includes('secret note'), false);
  }
  const task = nativeTask('appkit-save-note', '/tmp/none.json');
  const after = requestFor('appkit-save-note', [performed(task, 1, 'ax:text_input:note:set:note')]);
  assert.equal(
    byId(after)['ax:button:save-note'].description,
    'Press the button labeled "Save note". The task still requires this 1 more time(s).'
  );
  assert.deepEqual(after.progress.map((item: any) => item.done), [1, 0]);
});

test('a completed step says so and unordered steps have no precondition', () => {
  const task = nativeTask('appkit-counter', '/tmp/none.json');
  const done = requestFor('appkit-counter', [1, 2, 3].map((step) => performed(task, step, 'ax:button:increment')));
  assert.match(byId(done)['ax:button:increment'].description, /already did this the 3 time\(s\) the task requires\.$/);
  const size = requestFor('appkit-choose-size', []);
  assert.equal(byId(size)['ax:checkbox:i-agree'].description.includes('only after'), false);
  assert.match(byId(size)['ax:checkbox:i-agree'].description, /still requires this 1 more time\(s\)\.$/);
});

test('a task without steps sends no progress', () => {
  const task = nativeTask(CANVAS_TASK_ID, '/tmp/none.json');
  assert.deepEqual(task.steps, []);
  assert.deepEqual(task.progress([]), []);
});

test('recorded Python requests are reproduced exactly', () => {
  for (const item of fixture('native/native-progress-requests-v2.json').cases) {
    const task = nativeTask(item.task, '/tmp/none.json');
    const taskSources = sources(task, 'appkit');
    const request = nativeChoiceRequest(task, taskSources, task.plan(taskSources), item.history);
    assert.deepEqual(request, item.request, `${item.task} after ${item.history.length} steps`);
    validateRequest(request);
  }
});

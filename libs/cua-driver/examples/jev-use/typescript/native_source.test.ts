// NativeAccessibilitySource, stable IDs, policy, and AppKit tasks (RFC #4268).
// Mirrors python/tests/test_native_source.py.
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { validateRequest } from './choose_action.js';
import { chooseMockForTask } from './jev_adapter.js';
import { eligibleControls, parseWindowState, riskCategories, slug } from './native.js';
import type { Platform } from './native_roles.js';
import {
  MAX_EXECUTABLE_CANDIDATES,
  OracleError,
  appkitTask,
  compose,
  nativeChoiceRequest,
  type NativeTask,
} from './native_tasks.js';
import { immutableCandidate, NativeAccessibilitySource, type Candidate } from './sources.js';
import type { TaskSources } from './tasks.js';

function fixture(name: string): any {
  return JSON.parse(readFileSync(new URL(`../fixtures/native/${name}`, import.meta.url), 'utf8'));
}
const windowState = (name = 'initial') => fixture(`appkit-window-state-${name}-v1.json`);
const observe = (payload: any) => parseWindowState(payload, payload.pid, payload.window_id);

function sources(task: NativeTask, name = 'initial', foreground: string[] = []): TaskSources {
  return {
    ax: NativeAccessibilitySource.fromObservation(observe(windowState(name)), 'macos', {
      redact: task.redactText,
      textMethod: task.textMethod,
    }),
    visualPath: false,
    foregroundIds: new Set(foreground),
  };
}

test('shared ID, slug, and risk fixture matches Python', () => {
  const shared = fixture('native-candidate-ids-v1.json');
  for (const item of shared.cases) {
    const ids = eligibleControls(observe(item.window_state), item.platform as Platform).controls.map((c) => c.id);
    assert.deepEqual(ids, item.expected_ids, item.name);
  }
  for (const [label, expected] of Object.entries(shared.slugs)) assert.equal(slug(label), expected);
  for (const [label, expected] of Object.entries(shared.risks)) {
    assert.deepEqual([...riskCategories(label)].sort(), expected);
  }
});

test('AppKit fixture controls, exclusions, and ID stability', () => {
  const initial = eligibleControls(observe(windowState()), 'macos');
  const ids = initial.controls.map((control) => control.id);
  assert.deepEqual(ids, [
    'ax:button:increment',
    'ax:button:reset',
    'ax:button:click-target-left-right-double',
    'ax:checkbox:i-agree',
    'ax:button:right-click-for-context-menu',
    'ax:button:exit',
    'ax:text_input:note',
    'ax:button:save-note',
    'ax:radio:small',
    'ax:radio:medium',
    'ax:radio:large',
  ]);
  assert.ok(initial.excluded.off_screen > 0 && initial.excluded.unlabeled > 0 && initial.excluded.unknown_role > 0);
  const after = eligibleControls(observe(windowState('after-actions')), 'macos');
  assert.deepEqual(after.controls.map((control) => control.id), ids);
  assert.throws(() => parseWindowState(windowState(), 1, 2));
});

test('each element rule excludes', () => {
  const base = windowState();
  const window = base.elements[0];
  const make = (extra: Record<string, unknown>, drop?: string) => {
    const item: Record<string, unknown> = {
      element_index: 1, role: 'AXButton', depth: 1, parent_index: window.element_index, enabled: true,
      element_token: 's1:1', label: 'Go', frame: { x: window.frame.x + 5, y: window.frame.y + 5, w: 30, h: 20 }, ...extra,
    };
    if (drop) delete item[drop];
    return { ...base, elements: [window, item] };
  };
  const cases: [string, any][] = [
    ['unknown role', make({ role: 'AXStaticText' })],
    ['disabled', make({ enabled: false })],
    ['off screen', make({ frame: { x: 99999, y: 99999, w: 10, h: 10 } })],
    ['zero size', make({ frame: { x: window.frame.x, y: window.frame.y, w: 0, h: 10 } })],
    ['no frame', make({}, 'frame')],
    ['unlabeled', make({}, 'label')],
    ['label equals value', make({ role: 'AXTextField', label: 'typed', value: 'typed' })],
    ['marked unlabelled', make({ unlabelled: true })],
    ['web content', make({ in_web_content: true })],
    ['no token', make({}, 'element_token')],
  ];
  for (const [name, payload] of cases) {
    assert.deepEqual(eligibleControls(observe(payload), 'macos').controls, [], name);
  }
  assert.equal(eligibleControls(observe(make({})), 'macos').controls[0].id, 'ax:button:go');
});

test('compose orders by source, dedups, filters risk, and caps', () => {
  const cand = (id: string, source: 'page' | 'ax' | 'visual', risk: string[] = []): Candidate =>
    immutableCandidate({ id, description: 'd', tool: 'click', arguments: {}, source, risk: new Set(risk) });
  const ax = Array.from({ length: 30 }, (_, n) => cand(`ax:button:b${n}`, 'ax'));
  const { candidates, stats } = compose(
    {
      visual: [cand('v1', 'visual')],
      ax: [cand('ax:button:delete', 'ax', ['destructive']), ...ax],
      page: [cand('ax:button:b0', 'page'), cand('p1', 'page')],
    },
    new Set()
  );
  const ids = candidates.map((candidate) => candidate.id);
  assert.deepEqual(ids.slice(0, 3), ['ax:button:b0', 'p1', 'ax:button:b1']);
  assert.equal(candidates[0].source, 'page');
  assert.deepEqual(ids.slice(-2), ['reobserve', 'abstain']);
  assert.equal(ids.length, MAX_EXECUTABLE_CANDIDATES + 2);
  assert.equal(stats.duplicates, 1);
  assert.deepEqual(stats.risk_excluded, { destructive: 1 });
  assert.equal(stats.dropped, 32 - MAX_EXECUTABLE_CANDIDATES);
});

test('save-note task candidates, arguments, and redacted v2 request', () => {
  const task = appkitTask('appkit-save-note', '/tmp/none.json', { noteText: 'secret note' });
  const step = task.plan(sources(task));
  const byId = new Map(step.candidates.map((candidate) => [candidate.id, candidate]));
  assert.equal(byId.has('ax:button:reset'), false);
  assert.equal(byId.has('ax:button:exit'), false);
  const setter = byId.get('ax:text_input:note:set:note')!;
  assert.equal(setter.tool, 'set_value');
  assert.equal(setter.arguments.value, 'secret note');
  assert.equal(setter.arguments.element_token, 's00000086:21');
  assert.deepEqual({ ...byId.get('ax:button:save-note')!.arguments }, {
    pid: 20535, window_id: 71364, element_token: 's00000086:22', delivery_mode: 'background',
  });
  const request = nativeChoiceRequest(task, sources(task), step, []);
  validateRequest(request);
  const wire = JSON.stringify(request);
  assert.equal(wire.includes('secret note'), false);
  assert.equal(wire.includes('element_token'), false);
  assert.equal(wire.includes('s00000086:'), false);
});

test('satisfied controls leave the set and foreground needs permission', () => {
  const note = appkitTask('appkit-save-note', '/tmp/none.json', { noteText: 'hello jev' });
  assert.equal(note.candidates(sources(note, 'after-actions')).some((c) => c.id === 'ax:text_input:note:set:note'), false);
  const size = appkitTask('appkit-choose-size', '/tmp/none.json');
  const sizeIds = size.candidates(sources(size, 'after-actions')).map((c) => c.id);
  assert.equal(sizeIds.includes('ax:radio:large'), false);
  const denied = appkitTask('appkit-counter', '/tmp/none.json');
  const deniedIds = denied.candidates(sources(denied, 'initial', ['ax:button:increment'])).map((c) => c.id);
  assert.equal(deniedIds.some((id) => id.startsWith('ax:button:increment')), false);
  const allowed = appkitTask('appkit-counter', '/tmp/none.json', { allowForeground: true });
  const fg = allowed
    .candidates(sources(allowed, 'initial', ['ax:button:increment']))
    .find((c) => c.id === 'ax:button:increment:foreground')!;
  assert.equal(fg.arguments.delivery_mode, 'foreground');
});

test('mock provider follows task preferences', () => {
  for (const [id, expected] of [
    ['appkit-counter', 'ax:button:increment'],
    ['appkit-save-note', 'ax:text_input:note:set:note'],
    ['appkit-choose-size', 'ax:radio:large'],
  ]) {
    const task = appkitTask(id, '/tmp/none.json');
    const taskSources = sources(task);
    assert.equal(chooseMockForTask(task, taskSources, task.candidates(taskSources), []).choice, expected);
  }
});

test('app-state oracle and classification', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'jev-native-'));
  const path = join(directory, 'state.json');
  const state = { schema: 'cua.appkit_task_state_v1', pid: 42, counter: 3, agreed: true, size: 'large', note_saved: 'n' };
  writeFileSync(path, JSON.stringify(state));
  const counter = appkitTask('appkit-counter', path, { pid: 42 });
  assert.equal(counter.classify(await counter.readOracle(), 3), 'verified');
  assert.equal(counter.classify({ ...state, counter: 4 }, 4), 'refuted');
  assert.equal(counter.classify({ ...state, counter: 1 }, 6), 'budget_exhausted');
  assert.equal(appkitTask('appkit-save-note', path, { noteText: 'm' }).classify(state, 2), 'refuted');
  assert.equal(appkitTask('appkit-choose-size', path).classify(state, 2), 'verified');
  await assert.rejects(appkitTask('appkit-counter', path, { pid: 43 }).readOracle(), OracleError);
  const entry = appkitTask('appkit-save-note', path, { noteText: 'topsecret' }).historyEntry(1, 'x', undefined, {
    outcome: 'typed topsecret',
  });
  assert.equal(entry.outcome, 'typed [note text]');
});

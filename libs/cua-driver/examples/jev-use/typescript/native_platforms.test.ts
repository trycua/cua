// Native tasks on the WPF and WinUI3 (Windows UIA) and GTK3 (Linux AT-SPI)
// harnesses (RFC #4268). Mirrors python/tests/test_native_platforms.py; the
// fixtures are real get_window_state results recorded by verify_native.py.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { validateRequest } from './choose_action.js';
import { chooseMockForTask } from './jev_adapter.js';
import { eligibleControls, parseWindowState } from './native.js';
import { RAW_ROLES, roleClass } from './native_roles.js';
import {
  HARNESSES,
  NATIVE_TASK_IDS,
  appkitTask,
  nativeChoiceRequest,
  nativeTask,
  splitTaskId,
  type NativeTask,
} from './native_tasks.js';
import { NativeAccessibilitySource } from './sources.js';
import type { TaskSources } from './tasks.js';

const PLATFORM_HARNESSES = ['wpf', 'winui3', 'gtk3'] as const;

function windowState(harness: string, name: string): any {
  const url = new URL(`../fixtures/native/${harness}-window-state-${name}-v1.json`, import.meta.url);
  return JSON.parse(readFileSync(url, 'utf8'));
}
const observe = (payload: any) => parseWindowState(payload, payload.pid, payload.window_id);

function sources(task: NativeTask, harness: string, name: string): TaskSources {
  return {
    ax: NativeAccessibilitySource.fromObservation(observe(windowState(harness, name)), HARNESSES[harness].platform, {
      redact: task.redactText,
      textMethod: task.textMethod,
    }),
    visualPath: false,
    foregroundIds: new Set(),
  };
}

test('every harness has the same three tasks', () => {
  assert.deepEqual(
    NATIVE_TASK_IDS,
    [
      ...['appkit', 'wpf', 'winui3', 'gtk3'].flatMap((h) => ['counter', 'save-note', 'choose-size'].map((k) => `${h}-${k}`)),
      'canvas-cancel',
    ]
  );
  for (const taskId of NATIVE_TASK_IDS) {
    if (taskId === 'canvas-cancel') continue; // not a form harness (see native_canvas.test.ts)
    const [harness, kind] = splitTaskId(taskId);
    const task = nativeTask(taskId, '/tmp/none.json', { pid: 7 });
    assert.equal(task.scope.windowTitle, harness.windowTitle);
    assert.equal(task.oracle.schema, harness.stateSchema);
    assert.equal(task.oracle.expectedPid, 7);
    assert.deepEqual(task.mockPreferences, nativeTask(`appkit-${kind}`, '/tmp/none.json').mockPreferences);
  }
  assert.equal(HARNESSES.wpf.windowTitle, 'CuaTestHarness WPF Tasks');
  assert.equal(HARNESSES.winui3.windowTitle, 'CuaTestHarness WinUI3 Tasks');
  assert.equal(HARNESSES.winui3.stateEnv, 'CUA_WINUI3_TASK_STATE');
  assert.equal(HARNESSES.winui3.processName, 'CuaTestHarness.WinUI3');
  assert.equal(HARNESSES.gtk3.windowTitle, 'CuaTestHarness GTK3 Tasks');
  for (const taskId of ['wpf-reset', 'winui3-exit', 'uwp-counter', 'counter', '']) {
    assert.throws(() => splitTaskId(taskId));
  }
  assert.throws(() => appkitTask('wpf-counter', '/tmp/none.json'));
});

test('recorded task controls map to the same candidate IDs', () => {
  for (const harness of PLATFORM_HARNESSES) {
    const note = nativeTask(`${harness}-save-note`, '/tmp/none.json', { noteText: 'secret note' });
    const noteSources = sources(note, harness, 'initial');
    const step = note.plan(noteSources);
    const ids = step.candidates.map((c) => c.id);
    assert.deepEqual(ids.slice(-2), ['reobserve', 'abstain'], harness);
    for (const expected of ['ax:text_input:note:set:note', 'ax:button:save-note', 'ax:button:increment']) {
      assert.ok(ids.includes(expected), `${harness} ${expected}`);
    }
    assert.ok(!ids.includes('ax:button:reset') && !ids.includes('ax:button:exit'), harness);
    const setter = step.candidates.find((c) => c.id === 'ax:text_input:note:set:note')!;
    assert.equal(setter.tool, 'set_value');
    assert.equal(setter.arguments.value, 'secret note');
    const request = nativeChoiceRequest(note, noteSources, step, []);
    validateRequest(request);
    const wire = JSON.stringify(request);
    assert.ok(!wire.includes('secret note') && !wire.includes('element_token'), harness);
    assert.ok(!wire.includes(`${noteSources.ax!.observation.snapshotId}:`), harness);

    const size = nativeTask(`${harness}-choose-size`, '/tmp/none.json');
    const initial = sources(size, harness, 'initial');
    const sizeIds = size.candidates(initial).map((c) => c.id);
    for (const expected of ['ax:radio:small', 'ax:radio:medium', 'ax:radio:large', 'ax:checkbox:i-agree']) {
      assert.ok(sizeIds.includes(expected), `${harness} ${expected}`);
    }
    assert.equal(chooseMockForTask(size, initial, size.candidates(initial), []).choice, 'ax:radio:large');
    const after = sources(size, harness, 'after-choose-size');
    const controls = new Map(after.ax!.controls.map((c) => [c.id, c]));
    assert.equal(controls.get('ax:radio:large')?.selected, true, harness);
    assert.equal(controls.get('ax:checkbox:i-agree')?.selected, true, harness);
    const afterIds = size.candidates(after).map((c) => c.id);
    assert.ok(!afterIds.includes('ax:radio:large') && afterIds.includes('ax:radio:small'), harness);
  }
});

test('a text entry value is never a label', () => {
  for (const harness of PLATFORM_HARNESSES) {
    const note = nativeTask(`${harness}-save-note`, '/tmp/none.json');
    const after = sources(note, harness, 'after-save-note');
    const field = after.ax!.controls.find((c) => c.roleClass === 'text_input')!;
    assert.equal(field.label, 'Note');
    const ids = note.candidates(after).map((c) => c.id);
    if (harness === 'gtk3') {
      // Known limitation of Cua Driver 0.30.2 (#4291): a named AT-SPI text
      // field reports no value, so the written note is not observable.
      assert.ok('limitation' in windowState(harness, 'after-save-note')._fixture);
      assert.equal(field.value, undefined);
      assert.ok(ids.includes('ax:text_input:note:set:note'));
      continue;
    }
    assert.equal(field.value, 'jev-use native note');
    assert.ok(!ids.includes('ax:text_input:note:set:note'));
  }
});

test('the Windows title bar is window chrome and GTK3 roles map', () => {
  for (const harness of ['wpf', 'winui3']) {
    const windows = eligibleControls(observe(windowState(harness, 'initial')), 'windows');
    assert.equal(windows.excluded.window_chrome, 4, harness);
    assert.deepEqual(
      windows.controls.map((c) => c.label),
      ['Increment', 'Reset', 'I agree', 'Small', 'Medium', 'Large', 'Note', 'Save note', 'Exit'],
      harness
    );
  }
  const gtk3 = eligibleControls(observe(windowState('gtk3', 'initial')), 'linux');
  assert.deepEqual(gtk3.excluded, {});
  assert.deepEqual(
    gtk3.controls.map((c) => [c.roleClass, c.label]),
    [
      ['button', 'Increment'], ['button', 'Reset'], ['checkbox', 'I agree'],
      ['radio', 'Small'], ['radio', 'Medium'], ['radio', 'Large'],
      ['text_input', 'Note'], ['button', 'Save note'], ['button', 'Exit'],
    ]
  );
});

test('WinUI3 roles map through the Windows table with no WinUI3-specific row', () => {
  // WinUI3's automation peers report the same UIA control types as WPF. Its
  // TextBlock appears as static Text (WPF's task window has none), which is an
  // unknown role and never a candidate.
  const windowsRows = new Set(Object.values(RAW_ROLES.windows).flat());
  for (const name of ['initial', 'after-save-note', 'after-choose-size']) {
    const roles = new Set<string>(windowState('winui3', name).elements.map((e: any) => e.role));
    assert.deepEqual([...roles].filter((r) => !windowsRows.has(r)).sort(), ['Text', 'TitleBar'], name);
  }
  assert.equal(roleClass('Text', 'windows'), null);
  const winui3 = eligibleControls(observe(windowState('winui3', 'initial')), 'windows');
  assert.deepEqual(winui3.excluded, { unknown_role: 2, window_chrome: 4 });
  assert.deepEqual(
    winui3.controls.map((c) => [c.roleClass, c.label]),
    [
      ['button', 'Increment'], ['button', 'Reset'], ['checkbox', 'I agree'],
      ['radio', 'Small'], ['radio', 'Medium'], ['radio', 'Large'],
      ['text_input', 'Note'], ['button', 'Save note'], ['button', 'Exit'],
    ]
  );
  const wpf = eligibleControls(observe(windowState('wpf', 'initial')), 'windows');
  assert.deepEqual(winui3.controls.map((c) => c.id), wpf.controls.map((c) => c.id));
});

// The visual fallback for views without application accessibility elements.
// Mirrors python/tests/test_native_canvas.py; the fixtures are real
// get_window_state results for the visual-only canvas on Linux and Windows.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { validateRequest } from './choose_action.js';
import type { VisualObservation, VisualRegion } from './core.js';
import { chooseMockForTask } from './jev_adapter.js';
import { hasApplicationElements, parseWindowState } from './native.js';
import type { Platform } from './native_roles.js';
import {
  CANVAS,
  CANVAS_TASK_ID,
  HARNESSES,
  NATIVE_TASK_IDS,
  nativeChoiceRequest,
  nativeTask,
  splitTaskId,
  visualFallbackReason,
} from './native_tasks.js';
import { NativeAccessibilitySource, VisualRegionSource } from './sources.js';
import type { TaskSources } from './tasks.js';

const CANVAS_FIXTURES = { linux: 'canvas-linux', windows: 'canvas-windows', macos: 'canvas-macos' } as const;

function windowState(name: string): any {
  const url = new URL(`../fixtures/native/${name}-window-state-initial-v1.json`, import.meta.url);
  return JSON.parse(readFileSync(url, 'utf8'));
}
const observe = (payload: any) => parseWindowState(payload, payload.pid, payload.window_id);

function canvasSources(
  platform: keyof typeof CANVAS_FIXTURES,
  { visual = true, foregroundIds = new Set<string>() }: { visual?: boolean; foregroundIds?: Set<string> } = {}
): TaskSources {
  const observed = observe(windowState(CANVAS_FIXTURES[platform]));
  const ax = NativeAccessibilitySource.fromObservation(observed, platform);
  if (!visual) return { ax, visualPath: false, foregroundIds };
  const cards: [string, number][] = [['Save', 130], ['Send', 350], ['Cancel', 570]];
  const regions: VisualRegion[] = [
    ...cards.map(([text, x], index) => ({
      id: `r${index}`, kind: 'text' as const, text, confidence: 0.95, interactive: false, x, y: 250, width: 80, height: 30,
    })),
    { id: 'r9', kind: 'text', text: 'CHOOSE A SIGNAL', confidence: 0.95, interactive: false, x: 40, y: 30, width: 300, height: 30 },
  ];
  const parsed: VisualObservation = {
    captureId: observed.captureId ?? '', screenshotReference: 'ref', screenshotWidth: 762, screenshotHeight: 492,
    pid: observed.pid, windowId: observed.windowId, screenshotToAction: [1, 0, 0, 1, 0, 0], regions,
  };
  return { ax, visual: new VisualRegionSource(parsed, 'background', true), visualPath: true, foregroundIds };
}

test('recorded canvas trees have no application elements', () => {
  for (const [platform, name] of Object.entries(CANVAS_FIXTURES) as [Platform, string][]) {
    const observed = observe(windowState(name));
    assert.equal(observed.complete, false, platform);
    assert.equal(observed.truncated, false, platform);
    assert.equal(hasApplicationElements(observed, platform), false, platform);
    assert.deepEqual(NativeAccessibilitySource.fromObservation(observed, platform).controls, [], platform);
  }
  for (const harness of ['appkit', 'wpf', 'winui3', 'gtk3']) {
    assert.equal(hasApplicationElements(observe(windowState(harness)), HARNESSES[harness].platform), true, harness);
  }
  // Linux has no chrome rule, so the same title-bar buttons count as application elements.
  assert.equal(hasApplicationElements(observe(windowState('canvas-windows')), 'linux'), true);
  // The macOS menu bar and unlabeled window buttons are content everywhere else.
  assert.equal(hasApplicationElements(observe(windowState('canvas-macos')), 'linux'), true);
  const mac = windowState('canvas-macos');
  for (const extra of [
    [{ element_index: 90, parent_index: 0, role: 'AXButton', label: 'Save' }],
    [{ element_index: 92, parent_index: 0, role: 'AXGroup' }],
    [{ element_index: 92, parent_index: 0, role: 'AXGroup' }, { element_index: 91, parent_index: 92, role: 'AXButton' }],
  ]) {
    assert.equal(hasApplicationElements(observe({ ...mac, elements: [...mac.elements, ...extra] }), 'macos'), true);
  }
});

test('canvas trees fall back to visual regions; partial, truncated, and form cases do not', () => {
  const task = nativeTask(CANVAS_TASK_ID, '/tmp/none.json');
  for (const platform of ['linux', 'windows', 'macos'] as const) {
    assert.equal(visualFallbackReason(canvasSources(platform, { visual: false }), task, 0), 'no_application_elements');
  }
  const appkit = NativeAccessibilitySource.fromObservation(observe(windowState('appkit')), 'macos');
  assert.equal(visualFallbackReason({ ax: appkit, visualPath: false }, task, 0), undefined);
  const payload = windowState('canvas-linux');
  for (const change of [{ truncated: true }, { capture_id: null }]) {
    const ax = NativeAccessibilitySource.fromObservation(observe({ ...payload, ...change }), 'linux');
    assert.equal(visualFallbackReason({ ax, visualPath: false }, task, 0), undefined, JSON.stringify(change));
  }
  const form = nativeTask('gtk3-counter', '/tmp/none.json');
  assert.equal(visualFallbackReason(canvasSources('linux', { visual: false }), form, 0), undefined);
});

test('the canvas task is registered and bound to its state file', () => {
  assert.ok(NATIVE_TASK_IDS.includes(CANVAS_TASK_ID));
  assert.deepEqual(splitTaskId(CANVAS_TASK_ID), [CANVAS, 'cancel']);
  const task = nativeTask(CANVAS_TASK_ID, '/tmp/none.json', { pid: 7 });
  assert.equal(task.scope.windowTitle, 'Cua Visual-Only Canvas Fixture');
  assert.equal(task.oracle.schema, 'cua.visual_canvas_task_state_v1');
  assert.equal(task.oracle.expectedPid, 7);
  assert.deepEqual(task.scope.maxDepth, 1);
  assert.equal(task.visualMinConfidence, 0.8);
  assert.equal(nativeTask('appkit-counter', '/tmp/none.json').visualMinConfidence, 0.8);
  const sources = canvasSources('macos');
  const low = { ...sources.visual!.observation, regions: sources.visual!.observation.regions.map((r) => ({ ...r, confidence: 0.75 })) };
  assert.equal(new VisualRegionSource(low, 'background', true).find('button', 'Save'), undefined);
  assert.ok(new VisualRegionSource(low, 'background', true, 0.7).find('button', 'Save'));
});

test('only the Cancel region is executable, through a capture-bound click', () => {
  const task = nativeTask(CANVAS_TASK_ID, '/tmp/none.json');
  for (const platform of ['linux', 'windows', 'macos'] as const) {
    const sources = canvasSources(platform);
    const step = task.plan(sources);
    assert.deepEqual(step.candidates.map((c) => c.id), ['visual:cancel', 'reobserve', 'abstain'], platform);
    const save = step.candidates[0];
    assert.equal(save.source, 'visual');
    assert.equal(save.tool, 'click');
    assert.equal(save.arguments.delivery_mode, 'background');
    assert.equal(save.arguments.capture_id, sources.ax!.observation.captureId);
    assert.deepEqual([save.arguments.x, save.arguments.y], [610, 265]);
    const request = nativeChoiceRequest(task, sources, step, []);
    validateRequest(request);
    assert.deepEqual((request.elements as unknown[]) ?? [], []);
    assert.equal(chooseMockForTask(task, sources, step.candidates, []).choice, 'visual:cancel');
  }
});

test('a background refusal offers an explicit foreground variant only when allowed', () => {
  const refused = new Set(['visual:cancel']);
  const allowed = nativeTask(CANVAS_TASK_ID, '/tmp/none.json', { allowForeground: true });
  const step = allowed.plan(canvasSources('linux', { foregroundIds: refused }));
  assert.deepEqual(step.candidates.map((c) => c.id), ['visual:cancel:foreground', 'reobserve', 'abstain']);
  assert.equal(step.candidates[0].arguments.delivery_mode, 'foreground');
  const denied = nativeTask(CANVAS_TASK_ID, '/tmp/none.json');
  assert.deepEqual(
    denied.plan(canvasSources('linux', { foregroundIds: refused })).candidates.map((c) => c.id),
    ['reobserve', 'abstain']
  );
});

test('the canvas oracle', () => {
  const task = nativeTask(CANVAS_TASK_ID, '/tmp/none.json');
  assert.equal(task.check({ selected: null, action_count: 0 }), 'pending');
  assert.equal(task.check({ selected: 'cancel', action_count: 1 }), 'verified');
  assert.equal(task.check({ selected: 'cancel', action_count: 2 }), 'refuted');
  assert.equal(task.check({ selected: 'save', action_count: 1 }), 'refuted');
  assert.equal(task.check({ selected: 'send', action_count: 1 }), 'refuted');
});

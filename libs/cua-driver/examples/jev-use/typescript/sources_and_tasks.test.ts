import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { REDACTED_TOKEN, parseVisualRegions, type VisualObservation } from './core.js';
import { BrowserSemanticSource, VisualRegionSource, type CandidateSource } from './sources.js';
import {
  FIXTURE_GOAL,
  FixtureFormTask,
  SUBMIT_IDS,
  fixtureSources,
  type Task,
  type TaskSources,
} from './tasks.js';

const load = (name: string) =>
  JSON.parse(readFileSync(new URL(`../fixtures/${name}`, import.meta.url), 'utf8'));
const PAGE = load('jev-page-structure-replay-v1.json');
const VISUAL = load('jev-visual-replay-v1.json');
const PAYLOAD = VISUAL.visual_regions;

function observation(): VisualObservation {
  const source = PAYLOAD.capture.source;
  return parseVisualRegions(PAYLOAD, PAYLOAD.capture.capture_id, source.pid, source.window_id);
}

test('sources implement one interface with distinct kinds', () => {
  const page: CandidateSource = new BrowserSemanticSource(PAGE.snapshots.before_typing);
  const visual: CandidateSource = new VisualRegionSource(observation());
  assert.deepEqual([page.kind, visual.kind], ['page', 'visual']);
});

test('page source addresses refs with exact browser arguments', () => {
  const snapshot = PAGE.snapshots.after_typing;
  const page = new BrowserSemanticSource(snapshot);
  const button = page.find('button', 'Submit');
  assert.ok(button);
  assert.deepEqual([button.source, button.handle.ref], ['page', 'p4:2']);
  const candidate = page.click(button, 'c', 'd');
  assert.equal(candidate.tool, 'browser_click');
  assert.deepEqual(candidate.arguments, {
    target_id: snapshot.target_id,
    tab_id: snapshot.tab_id,
    ref: 'p4:2',
    input_route: 'dom_event',
  });
  assert.equal(page.find('button', 'Cancel'), undefined);
});

test('visual source clicks only capture-bound and never types', () => {
  const visual = observation();
  const source = new VisualRegionSource(visual);
  const submit = source.find('button', 'submit');
  assert.ok(submit);
  assert.equal(source.click(submit, 'c', 'd'), undefined);
  assert.equal(source.typeText(), undefined);
  const bound = new VisualRegionSource(visual, 'foreground', true);
  const candidate = bound.click(submit, 'c', 'd');
  assert.equal(candidate?.tool, 'click');
  assert.equal(candidate?.captureId, visual.captureId);
  assert.equal(candidate?.arguments.delivery_mode, 'foreground');
  assert.deepEqual(
    [candidate?.arguments.x, candidate?.arguments.y],
    [submit.handle.x + submit.handle.width / 2, submit.handle.y + submit.handle.height / 2]
  );
});

test('built-in task declares its spec', () => {
  const task: Task = new FixtureFormTask('secret-token', 'http://127.0.0.1:9/', 3);
  assert.equal(task.goal, FIXTURE_GOAL);
  assert.equal(task.maxSteps, 3);
  assert.equal(task.completionCandidateIds, SUBMIT_IDS);
  assert.deepEqual([...task.allowedActionKinds].sort(), ['browser_click', 'browser_type', 'click']);
  assert.equal(task.parameters.length, 1);
  const [parameter] = task.parameters;
  assert.equal(parameter.secret, true);
  assert.deepEqual([parameter.value, parameter.redaction], ['secret-token', REDACTED_TOKEN]);
  assert.deepEqual(task.redact({ a: ['x secret-token y'] }), { a: [`x ${REDACTED_TOKEN} y`] });
});

test('oracle classification uses the task budget', () => {
  const task = new FixtureFormTask('t', undefined, 2);
  assert.equal(task.classify({ submitted: 't' }, 0), 'verified');
  assert.equal(task.classify({ submitted: 'other' }, 0), 'refuted');
  assert.equal(task.classify({ submitted: null }, 1), 'unknown');
  assert.equal(task.classify({ submitted: null }, 2), 'budget_exhausted');
});

test('candidates outside the allowed action kinds are refused', () => {
  const task = new FixtureFormTask(PAGE.token);
  (task as { allowedActionKinds: ReadonlySet<string> }).allowedActionKinds = new Set([
    'browser_type',
  ]);
  assert.throws(() => task.candidates(fixtureSources(PAGE.snapshots.after_typing)), /browser_click/);
  assert.equal(task.candidates(fixtureSources(PAGE.snapshots.before_typing))[0].tool, 'browser_type');
});

test('state summary reads the step sources', () => {
  const task = new FixtureFormTask(VISUAL.token);
  const after = VISUAL.snapshots.after_typing;
  const pending: TaskSources = { page: new BrowserSemanticSource(after), visualPath: true };
  assert.equal(task.stateSummary(pending).submit_button, 'visual_check_pending');
  const parsed: TaskSources = {
    page: new BrowserSemanticSource(after),
    visual: new VisualRegionSource(observation()),
    visualPath: true,
  };
  assert.deepEqual(task.stateSummary(parsed), {
    verification_field: 'contains_required_token',
    submit_button: 'visual_only',
  });
});

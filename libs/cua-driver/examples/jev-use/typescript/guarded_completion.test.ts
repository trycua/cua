import assert from 'node:assert/strict';
import test from 'node:test';

import { planGuardedCompletion, resolveGuardedCompletion } from './guarded_completion.js';
import { immutableCandidate, type Candidate } from './sources.js';
import { FixtureFormTask, fixtureSources, type Task } from './tasks.js';
import type { BrowserSnapshot } from './core.js';

function snapshot(value: string, submitRefs: string[]): BrowserSnapshot {
  return {
    target_id: 'target',
    tab_id: 'tab',
    refs: [
      { role: 'textbox', name: 'verification value', ref: 'p1:0', value },
      ...submitRefs.map((ref) => ({ role: 'button', name: 'Submit', ref })),
    ],
  };
}

test('guarded completion returns the fresh candidate and exact redacted accepted telemetry', () => {
  const task = new FixtureFormTask('proof');
  const initial = fixtureSources(snapshot('', ['p1:1']));
  const selected = task.candidates(initial)[0];
  const plan = planGuardedCompletion(task, initial, selected, 'session-a');
  assert.ok(plan);

  const fresh = fixtureSources(snapshot('proof', ['p2:1']));
  const candidates = task.candidates(fresh);
  const completion = resolveGuardedCompletion(plan, task, fresh, candidates, 'session-a');
  assert.deepEqual(completion, {
    candidate: candidates[0],
    telemetry: {
      status: 'accepted',
      prior_ref: 'p1:1',
      fresh_ref: 'p2:1',
      verification_field: 'contains_required_token',
      submit_matches: 1,
      session: 'session-a',
    },
  });
  assert.equal(completion.candidate, candidates[0]);
  assert.notEqual(completion.candidate.arguments.ref, plan.priorRef);
  assert.ok(!JSON.stringify(completion.telemetry).includes(task.token));
});

function plannedCompletion() {
  const task = new FixtureFormTask('proof');
  const initial = fixtureSources(snapshot('', ['p1:1']));
  const plan = planGuardedCompletion(task, initial, task.candidates(initial)[0], 'session-a');
  assert.ok(plan);
  const fresh = fixtureSources(snapshot('proof', ['p2:1']));
  return { task, initial, plan, fresh, candidates: task.candidates(fresh) };
}

for (const session of ['', 'session-b']) {
  test(`guarded completion declines session ${JSON.stringify(session)} before other checks`, () => {
    const { task, plan, fresh, candidates } = plannedCompletion();
    const expected = {
      candidate: undefined,
      telemetry: { status: 'declined', reason: 'session_mismatch' },
    };
    assert.deepEqual(resolveGuardedCompletion(plan, task, fresh, candidates, session), expected);
    const otherTask: Task = Object.assign(Object.create(task), { id: 'other-task' });
    assert.deepEqual(
      resolveGuardedCompletion(plan, otherTask, { visualPath: false }, [], session),
      expected
    );
  });
}

test('guarded completion declines a different task before checking page presence', () => {
  const { task, plan, fresh, candidates } = plannedCompletion();
  const otherTask: Task = Object.assign(Object.create(task), { id: 'other-task' });
  const expected = {
    candidate: undefined,
    telemetry: { status: 'declined', reason: 'task_mismatch' },
  };
  assert.deepEqual(
    resolveGuardedCompletion(plan, otherTask, fresh, candidates, 'session-a'),
    expected
  );
  assert.deepEqual(
    resolveGuardedCompletion(plan, otherTask, { visualPath: false }, [], 'session-a'),
    expected
  );
});

test('guarded completion declines a missing page without reading task state', () => {
  const { task, plan } = plannedCompletion();
  assert.deepEqual(resolveGuardedCompletion(plan, task, { visualPath: false }, [], 'session-a'), {
    candidate: undefined,
    telemetry: { status: 'declined', reason: 'page_missing' },
  });
});

for (const value of ['', 'other', undefined]) {
  test(`guarded completion declines unproven field ${JSON.stringify(value)} before targets`, () => {
    const { task, plan } = plannedCompletion();
    const observed = snapshot(value ?? '', []);
    if (value === undefined) observed.refs = [];
    const sources = fixtureSources(observed);
    assert.deepEqual(
      resolveGuardedCompletion(plan, task, sources, task.candidates(sources), 'session-a'),
      { candidate: undefined, telemetry: { status: 'declined', reason: 'field_not_proven' } }
    );
  });
}

for (const refs of [[], ['p2:1', 'p2:2'], ['p1:1', 'p1:1'], ['']]) {
  test(`guarded completion declines non-unique submit refs ${JSON.stringify(refs)} before reuse`, () => {
    const { task, plan } = plannedCompletion();
    const fresh = fixtureSources(snapshot('proof', refs));
    assert.deepEqual(resolveGuardedCompletion(plan, task, fresh, [], 'session-a'), {
      candidate: undefined,
      telemetry: { status: 'declined', reason: 'submit_not_unique' },
    });
  });
}

test('guarded completion declines old-ref reuse before checking executable candidates', () => {
  const { task, plan } = plannedCompletion();
  const reused = fixtureSources(snapshot('proof', [plan.priorRef]));
  assert.deepEqual(resolveGuardedCompletion(plan, task, reused, [], 'session-a'), {
    candidate: undefined,
    telemetry: { status: 'declined', reason: 'ref_reused' },
  });
});

const nonExecutable: [string, Partial<Candidate>][] = [
  ['wrong ID', { id: 'another-submit' }],
  ['wrong tool', { tool: 'browser_type' }],
  ['visual tool', { tool: 'click' }],
  ['missing tool', { tool: null }],
  ['visual source', { source: 'visual' }],
  ['native source', { source: 'ax' }],
  ['missing source', { source: undefined }],
];

for (const [name, overrides] of nonExecutable) {
  test(`guarded completion declines a candidate with ${name}`, () => {
    const { task, plan, fresh, candidates } = plannedCompletion();
    const bad = immutableCandidate({ ...candidates[0], ...overrides });
    assert.deepEqual(resolveGuardedCompletion(plan, task, fresh, [bad], 'session-a'), {
      candidate: undefined,
      telemetry: { status: 'declined', reason: 'candidate_not_unique' },
    });
  });
}

for (const count of [0, 2]) {
  test(`guarded completion declines ${count} executable candidates before ref matching`, () => {
    const { task, plan, fresh, candidates } = plannedCompletion();
    const bad = immutableCandidate({ ...candidates[0], arguments: { ref: plan.priorRef } });
    assert.deepEqual(
      resolveGuardedCompletion(plan, task, fresh, Array<Candidate>(count).fill(bad), 'session-a'),
      { candidate: undefined, telemetry: { status: 'declined', reason: 'candidate_not_unique' } }
    );
  });
}

for (const ref of ['p1:1', 'p2:2', '', undefined, null, 2, { ref: 'p2:1' }]) {
  test(`guarded completion declines mismatched or malformed candidate ref ${JSON.stringify(ref)}`, () => {
    const { task, plan, fresh, candidates } = plannedCompletion();
    const bad = immutableCandidate({
      ...candidates[0],
      arguments: ref === undefined ? {} : { ref },
    });
    assert.deepEqual(resolveGuardedCompletion(plan, task, fresh, [bad], 'session-a'), {
      candidate: undefined,
      telemetry: { status: 'declined', reason: 'candidate_mismatch' },
    });
  });
}

test('guarded completion still requires a unique initial target', () => {
  const { task } = plannedCompletion();
  for (const refs of [[], ['p1:1', 'p1:2']]) {
    const sources = fixtureSources(snapshot('', refs));
    assert.equal(
      planGuardedCompletion(task, sources, task.candidates(sources)[0], 'session-a'),
      undefined
    );
  }
});
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  buildCandidates,
  formState,
  historyEntry,
  redactToken,
  REDACTED_TOKEN,
  validateChoice,
  type BrowserSnapshot,
  type HistoryEntry,
  type VisualObservation,
} from './core.js';
import { chooseMockAdapter, chooseWithTypeSafe, decisionState } from './jev_adapter.js';

const FIXTURE = JSON.parse(
  readFileSync(new URL('../fixtures/jev-page-structure-replay-v1.json', import.meta.url), 'utf8')
);
const TOKEN: string = FIXTURE.token;
const BEFORE: BrowserSnapshot = FIXTURE.snapshots.before_typing;
const AFTER: BrowserSnapshot = FIXTURE.snapshots.after_typing;

type RecordedStep = { step: number; selected_id: string; probabilities: Record<string, number> };

function withFieldValue(value: string | null): BrowserSnapshot {
  const snapshot = structuredClone(BEFORE);
  for (const ref of snapshot.refs ?? []) {
    if (ref.role === 'textbox' && ref.name === 'verification value') ref.value = value;
  }
  return snapshot;
}

function replayClient(recorded: RecordedStep[]) {
  const queue = [...recorded];
  const requests: any[] = [];
  return {
    requests,
    systemOne: async (request: any) => {
      requests.push(request);
      const step = queue.shift()!;
      return {
        answers: {
          driver_action: {
            type: 'choice' as const,
            choice: step.selected_id,
            confidence: Math.max(...Object.values(step.probabilities)),
            probabilities: step.probabilities,
          },
        },
      };
    },
  };
}

test('form state reports field status without the value', () => {
  assert.deepEqual(formState(BEFORE, TOKEN), {
    verification_field: 'empty',
    submit_button: 'available',
  });
  assert.equal(formState(AFTER, TOKEN).verification_field, 'contains_required_token');
  assert.equal(
    formState(withFieldValue('something-else'), TOKEN).verification_field,
    'contains_other_value'
  );
  const noButton = { ...AFTER, refs: (AFTER.refs ?? []).filter((ref) => ref.role !== 'button') };
  assert.equal(formState(noButton, TOKEN).submit_button, 'not_in_page_structure');
  assert.deepEqual(formState({ target_id: 't', tab_id: 't', refs: [] }, TOKEN), {
    verification_field: 'not_found',
    submit_button: 'not_in_page_structure',
  });
});

test('redaction is nested and deterministic', () => {
  const value = { a: [`x ${TOKEN} y`, { b: TOKEN }], n: 3, none: null };
  assert.deepEqual(redactToken(value, TOKEN), {
    a: [`x ${REDACTED_TOKEN} y`, { b: REDACTED_TOKEN }],
    n: 3,
    none: null,
  });
  assert.equal(redactToken('unchanged', ''), 'unchanged');
});

test('pre-fix submit state hid readiness and leaked the token', () => {
  const old = FIXTURE.pre_fix_submit_state;
  assert.equal('form' in old.observation, false);
  assert.ok(old.observation.outline.includes(TOKEN));
  assert.ok(old.observation.outline.includes('status=waiting'));
  assert.ok('probabilities' in old.history[0]);
});

test('after typing the observation states the filled form', () => {
  const history = [historyEntry(1, 'type-verification-value')];
  const state = decisionState(AFTER, undefined, history, TOKEN);
  const encoded = JSON.stringify(state);
  assert.equal(encoded.includes(TOKEN), false);
  assert.deepEqual(JSON.parse(state.observation.form), {
    verification_field: 'contains_required_token',
    submit_button: 'available',
  });
  assert.ok(state.observation.outline.includes(`textbox "verification value": ${REDACTED_TOKEN}`));
  assert.deepEqual(JSON.parse(state.history), [
    {
      step: 1,
      selected_id: 'type-verification-value',
      outcome: 'typed the required token into the verification field',
    },
  ]);
  assert.equal(JSON.stringify(decisionState(AFTER, undefined, history, TOKEN)), encoded);

  const criteria = Object.fromEntries(
    buildCandidates(AFTER, TOKEN).map((candidate) => [candidate.id, candidate.description])
  );
  assert.deepEqual(Object.keys(criteria), ['submit-form', 'reobserve', 'abstain']);
  assert.ok(criteria['submit-form'].includes('already contains the required token'));
  assert.ok(criteria.reobserve.includes('stale or contradicts'));
  assert.equal(JSON.stringify(criteria).includes(TOKEN), false);
});

test('visual text is redacted too', () => {
  const visual: VisualObservation = {
    captureId: 'cap',
    screenshotReference: 'ref',
    screenshotWidth: 100,
    screenshotHeight: 100,
    pid: 7,
    windowId: 9,
    screenshotToAction: [1, 0, 0, 1, 0, 0],
    regions: [
      {
        id: 'r1',
        kind: 'text',
        text: TOKEN,
        confidence: 0.9,
        interactive: false,
        x: 1,
        y: 1,
        width: 10,
        height: 10,
      },
    ],
  };
  const state = decisionState(AFTER, visual, [], TOKEN);
  assert.equal(JSON.parse(state.observation.visual).regions[0].text, REDACTED_TOKEN);
  assert.equal(JSON.stringify(state).includes(TOKEN), false);
});

test('background refusal history is compact', () => {
  const entry = historyEntry(2, 'submit-form', 'background_unsupported');
  assert.deepEqual(Object.keys(entry).sort(), ['outcome', 'selected_id', 'step']);
  assert.ok(entry.outcome.includes('background_unsupported'));
});

test('mock provider types then submits over recorded snapshots', () => {
  const first = buildCandidates(BEFORE, TOKEN);
  assert.equal(
    chooseMockAdapter(first, BEFORE, undefined, [], TOKEN).choice,
    'type-verification-value'
  );
  const second = buildCandidates(AFTER, TOKEN);
  const history = [historyEntry(1, 'type-verification-value')];
  assert.equal(chooseMockAdapter(second, AFTER, undefined, history, TOKEN).choice, 'submit-form');
});

test('recorded live choices replay through the new state', async () => {
  for (const [name, run] of Object.entries<any>(FIXTURE.recorded_live_runs)) {
    const client = replayClient(run.steps);
    const history: HistoryEntry[] = [];
    let outcome = 'budget_exhausted';
    for (let step = 1; step <= 4; step += 1) {
      const snapshot = step === 1 ? BEFORE : AFTER;
      const candidates = buildCandidates(snapshot, TOKEN);
      const recorded: RecordedStep = run.steps[step - 1];
      assert.deepEqual(
        new Set(Object.keys(recorded.probabilities)),
        new Set(candidates.map((candidate) => candidate.id)),
        name
      );
      const answer = await chooseWithTypeSafe(
        client as never,
        candidates,
        snapshot,
        undefined,
        history,
        TOKEN
      );
      const candidate = validateChoice(answer.choice, candidates);
      const request = client.requests.at(-1);
      assert.equal(JSON.stringify(request.state).includes(TOKEN), false, name);
      assert.equal(
        JSON.parse(request.state.observation.form).verification_field,
        step === 1 ? 'empty' : 'contains_required_token',
        name
      );
      for (const item of JSON.parse(request.state.history)) {
        assert.deepEqual(Object.keys(item).sort(), ['outcome', 'selected_id', 'step'], name);
      }
      const criteria = request.questions.driver_action.criteria;
      assert.ok('reobserve' in criteria && 'abstain' in criteria, name);
      history.push(historyEntry(step, candidate.id));
      if (candidate.id === 'submit-form') {
        outcome = 'verified';
        break;
      }
    }
    assert.equal(outcome, run.outcome, name);
  }
});

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  SUBMIT_IDS,
  buildCandidates,
  chooseMock,
  formState,
  hasExecutableCandidate,
  historyEntry,
  parseVisualRegions,
  validateChoice,
  visualSubmitRegion,
  type BrowserSnapshot,
  type HistoryEntry,
  type VisualDelivery,
  type VisualObservation,
} from './core.js';
import { chooseWithTypeSafe, decisionState } from './jev_adapter.js';

const load = (name: string) =>
  JSON.parse(readFileSync(new URL(`../fixtures/${name}`, import.meta.url), 'utf8'));
const VISUAL = load('jev-visual-replay-v1.json');
const PAGE = load('jev-page-structure-replay-v1.json');
const TOKEN: string = VISUAL.token;
const BEFORE: BrowserSnapshot = VISUAL.snapshots.before_typing;
const AFTER: BrowserSnapshot = VISUAL.snapshots.after_typing;
const PAYLOAD = VISUAL.visual_regions;

type RecordedStep = {
  step: number;
  selected_id: string;
  probabilities: Record<string, number>;
  action_error?: string;
};

function observation(): VisualObservation {
  const source = PAYLOAD.capture.source;
  return parseVisualRegions(PAYLOAD, PAYLOAD.capture.capture_id, source.pid, source.window_id);
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

test('visual fixture exposes Submit only as text and as one visual region', () => {
  for (const snapshot of [BEFORE, AFTER]) {
    assert.equal(
      (snapshot.refs ?? []).some((ref) => ref.role === 'button'),
      false
    );
    assert.ok((snapshot.outline ?? '').includes('statictext "Submit"'));
  }
  assert.equal(visualSubmitRegion(observation())?.text, 'Submit');
});

test('submit state names the visual path instead of a missing button', () => {
  const visual = observation();
  assert.equal(formState(BEFORE, TOKEN).submit_button, 'not_in_page_structure');
  assert.deepEqual(formState(BEFORE, TOKEN, undefined, true), {
    verification_field: 'empty',
    submit_button: 'visual_check_pending',
  });
  assert.deepEqual(formState(AFTER, TOKEN, visual, true), {
    verification_field: 'contains_required_token',
    submit_button: 'visual_only',
  });
  const withoutSubmit = { ...visual, regions: visual.regions.filter((r) => r.text !== 'Submit') };
  assert.equal(formState(AFTER, TOKEN, withoutSubmit, true).submit_button, 'not_found_visually');
  const submit = visualSubmitRegion(visual)!;
  const duplicated = { ...visual, regions: [...visual.regions, { ...submit, id: 'dup' }] };
  assert.equal(formState(AFTER, TOKEN, duplicated, true).submit_button, 'not_found_visually');
  // A page-structure Submit ref always wins, with or without a visual path.
  assert.equal(
    formState(PAGE.snapshots.after_typing, PAGE.token, visual, true).submit_button,
    'available'
  );
});

test('pre-fix step-one state contradicted the outline', () => {
  const old = VISUAL.pre_fix_step1;
  assert.equal(old.state.observation.form.submit_button, 'not_in_page_structure');
  assert.ok(old.state.observation.outline.includes('statictext "Submit"'));
  assert.ok(old.criteria.reobserve.includes('incomplete'));

  const state = decisionState(BEFORE, undefined, [], TOKEN, true);
  assert.deepEqual(JSON.parse(state.observation.form), {
    verification_field: 'empty',
    submit_button: 'visual_check_pending',
  });
  assert.equal(state.observation.outline, old.state.observation.outline);

  const criteria = Object.fromEntries(
    buildCandidates(BEFORE, TOKEN).map((candidate) => [candidate.id, candidate.description])
  );
  assert.deepEqual(Object.keys(criteria), Object.keys(old.criteria));
  assert.equal(criteria.reobserve.includes('incomplete'), false);
  assert.ok(criteria.reobserve.includes('visual-only'));
  assert.ok(criteria.reobserve.includes('still pending'));
});

test('page-structure fixture types then submits with browser_click', () => {
  const first = buildCandidates(PAGE.snapshots.before_typing, PAGE.token);
  assert.equal(chooseMock(first).choice, 'type-verification-value');
  const second = buildCandidates(PAGE.snapshots.after_typing, PAGE.token);
  const choice = validateChoice(chooseMock(second).choice!, second);
  assert.deepEqual([choice.id, choice.tool], ['submit-form', 'browser_click']);
});

test('visual fixture types then submits with a capture-bound click', () => {
  const first = buildCandidates(BEFORE, TOKEN, undefined, true);
  const typed = validateChoice(chooseMock(first).choice!, first);
  assert.deepEqual([typed.id, typed.tool], ['type-verification-value', 'browser_type']);

  // After typing the page structure offers no action, so the runner parses.
  assert.equal(hasExecutableCandidate(buildCandidates(AFTER, TOKEN, undefined, true)), false);
  const visual = observation();
  const second = buildCandidates(AFTER, TOKEN, visual, true);
  const submit = validateChoice(chooseMock(second).choice!, second, visual.captureId);
  const region = visualSubmitRegion(visual)!;
  assert.deepEqual([submit.id, submit.tool], ['submit-form', 'click']);
  assert.equal(submit.arguments.capture_id, visual.captureId);
  assert.equal(submit.arguments.delivery_mode, 'background');
  assert.deepEqual(
    [submit.arguments.x, submit.arguments.y],
    [region.x + region.width / 2, region.y + region.height / 2]
  );

  const third = buildCandidates(AFTER, TOKEN, visual, true, 'foreground');
  const foreground = validateChoice(chooseMock(third).choice!, third, visual.captureId);
  assert.deepEqual(
    [foreground.id, foreground.arguments.delivery_mode],
    ['submit-form-foreground', 'foreground']
  );
});

test('recorded visual-run choices replay through the new state', async () => {
  const visual = observation();
  for (const [name, run] of Object.entries<any>(VISUAL.recorded_live_runs)) {
    const client = replayClient(run.steps);
    const history: HistoryEntry[] = [];
    let typed = false;
    let delivery: VisualDelivery = 'background';
    let outcome = 'budget_exhausted';
    for (const recorded of run.steps as RecordedStep[]) {
      const snapshot = typed ? AFTER : BEFORE;
      const current = typed ? visual : undefined;
      const candidates = buildCandidates(snapshot, TOKEN, current, true, delivery);
      assert.deepEqual(
        new Set(Object.keys(recorded.probabilities)),
        new Set(candidates.map((candidate) => candidate.id)),
        name
      );
      const answer = await chooseWithTypeSafe(
        client as never,
        candidates,
        snapshot,
        current,
        history,
        TOKEN,
        true
      );
      const candidate = validateChoice(answer.choice, candidates, current?.captureId);
      const request = client.requests.at(-1);
      assert.equal(JSON.stringify(request.state).includes(TOKEN), false, name);
      assert.deepEqual(
        JSON.parse(request.state.observation.form),
        {
          verification_field: typed ? 'contains_required_token' : 'empty',
          submit_button: typed ? 'visual_only' : 'visual_check_pending',
        },
        name
      );
      assert.equal(
        request.questions.driver_action.criteria.reobserve.includes('incomplete'),
        false
      );
      for (const item of JSON.parse(request.state.history)) {
        assert.deepEqual(Object.keys(item).sort(), ['outcome', 'selected_id', 'step'], name);
      }
      history.push(historyEntry(recorded.step, candidate.id, recorded.action_error));
      if (candidate.id === 'type-verification-value') {
        typed = true;
      } else if (SUBMIT_IDS.has(candidate.id)) {
        assert.equal(candidate.tool, 'click', name);
        if (recorded.action_error) {
          delivery = 'foreground';
        } else {
          outcome = 'verified';
          break;
        }
      }
    }
    assert.equal(outcome, run.outcome, name);
  }
});

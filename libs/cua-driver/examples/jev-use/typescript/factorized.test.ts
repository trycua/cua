import assert from 'node:assert/strict';
import test from 'node:test';

import {
  GATE_THRESHOLD,
  buildFactorizedQuestions,
  chooseFactorized,
  parseFactorizedDecision,
  stateDigest,
} from './factorized.js';
import { readJevConfig, type SystemOneTransport } from './jev_backends.js';

const CANDIDATES = {
  'type-verification-value': 'Replace the verification field.',
  reobserve: 'Obtain a fresh observation.',
  abstain: 'Stop without acting.',
};

function answers(choice = 'type-verification-value', confidence = 0.9, goal = 0.1, reobserve = 0.05) {
  const rest = (1 - confidence) / 2;
  return {
    selection: {
      type: 'choice',
      choice,
      confidence,
      probabilities: Object.fromEntries(
        Object.keys(CANDIDATES).map((id) => [id, id === choice ? confidence : rest])
      ),
    },
    goal_achieved: { type: 'noul', noul: goal },
    needs_reobserve: { type: 'noul', noul: reobserve },
  };
}

function stubTransport(payloadAnswers: Record<string, unknown>): SystemOneTransport {
  return async () => ({ model: 'stub-jev', answers: payloadAnswers });
}

test('builder produces the three questions with types', () => {
  const questions = buildFactorizedQuestions(CANDIDATES, 'Submit the form.');
  assert.deepEqual(Object.keys(questions).sort(), [
    'goal_achieved',
    'needs_reobserve',
    'selection',
  ]);
  assert.equal(questions.selection.type, 'choice');
  assert.equal(questions.goal_achieved.type, 'noul');
  assert.equal(questions.needs_reobserve.type, 'noul');
  assert.deepEqual(questions.selection.criteria, CANDIDATES);
});

test('builder requires a goal', () => {
  assert.throws(() => buildFactorizedQuestions(CANDIDATES, '  '), /goal/);
});

test('parser keeps the selection on the happy path', () => {
  const decision = parseFactorizedDecision(answers(), CANDIDATES, { backend: 'local' });
  assert.ok(decision);
  assert.equal(decision.selectedId, 'type-verification-value');
  assert.equal(decision.backend, 'local');
  assert.equal(decision.goalAchieved, 0.1);
});

test('goal_achieved gate fires abstain', () => {
  const decision = parseFactorizedDecision(answers('type-verification-value', 0.9, GATE_THRESHOLD + 0.1), CANDIDATES);
  assert.ok(decision);
  assert.equal(decision.selectedId, 'abstain');
});

test('needs_reobserve gate fires reobserve', () => {
  const decision = parseFactorizedDecision(answers('type-verification-value', 0.9, 0.1, GATE_THRESHOLD + 0.1), CANDIDATES);
  assert.ok(decision);
  assert.equal(decision.selectedId, 'reobserve');
});

test('gates need the reserved ids', () => {
  const noReserved = { a: 'Do A.', b: 'Do B.' };
  const raw = answers('a', 0.9, 0.99, 0.99);
  raw.selection.probabilities = { a: 0.9, b: 0.1 };
  const decision = parseFactorizedDecision(raw, noReserved);
  assert.ok(decision);
  assert.equal(decision.selectedId, 'a');
});

test('low confidence fails open', () => {
  assert.equal(parseFactorizedDecision(answers('type-verification-value', 0.1), CANDIDATES), null);
});

test('malformed answers fail open', () => {
  const badMass = answers();
  badMass.selection.probabilities.abstain = 0.9; // mass 1.81
  assert.equal(parseFactorizedDecision(badMass, CANDIDATES), null);
  const missing = answers() as Record<string, unknown>;
  delete missing.needs_reobserve;
  assert.equal(parseFactorizedDecision(missing, CANDIDATES), null);
  assert.equal(parseFactorizedDecision('nope', CANDIDATES), null);
  assert.equal(parseFactorizedDecision(answers(), { a: '' }), null);
});

test('state digest is deterministic, order-independent, and secret-free', () => {
  const first = stateDigest('Enter the token hunter2', ['a', 'b'], 'c1');
  const second = stateDigest('Enter the token hunter2', ['b', 'a'], 'c1');
  assert.equal(first, second);
  assert.ok(first.startsWith('sha256:'));
  assert.ok(!first.includes('hunter2'));
});

test('mock backend returns a packet', async () => {
  const packet = await chooseFactorized(readJevConfig({}), {
    goal: 'g',
    observation: {},
    candidates: CANDIDATES,
    captureId: 'c1',
  });
  assert.ok(packet);
  const body = packet.toDict();
  assert.equal(body.selected_id, 'type-verification-value');
  assert.equal(body.backend, 'mock');
  assert.ok(body.state_digest.startsWith('sha256:'));
  assert.ok(!JSON.stringify(body).toLowerCase().includes('screenshot'));
});

test('http backend returns a packet', async () => {
  const packet = await chooseFactorized(readJevConfig({ JEV_BACKEND: 'local' }), {
    goal: 'g',
    observation: { page: 'fixture' },
    candidates: CANDIDATES,
    captureId: 'c1',
    transport: stubTransport(answers()),
  });
  assert.ok(packet);
  assert.equal(packet.decision.selectedId, 'type-verification-value');
  assert.equal(packet.decision.backend, 'local');
  assert.ok(packet.latencyMs >= 0);
});

test('transport failure fails open', async () => {
  const transport: SystemOneTransport = async () => {
    throw new Error('boom');
  };
  assert.equal(
    await chooseFactorized(readJevConfig({ JEV_BACKEND: 'local' }), {
      goal: 'g',
      observation: {},
      candidates: CANDIDATES,
      transport,
    }),
    null
  );
});

test('bad criteria fail open', async () => {
  assert.equal(
    await chooseFactorized(readJevConfig({}), {
      goal: 'g',
      observation: {},
      candidates: {},
    }),
    null
  );
});

import assert from 'node:assert/strict';
import test from 'node:test';
import { TypeSafeClient } from '@typesafe-ai/sdk';

import { buildCandidates, chooseMock, classify, validateChoice } from './core.js';
import { chooseWithTypeSafe, Driver, selectTabId, validateFixtureUrl } from './run.js';

function snapshot(value: string | null = null) {
  return {
    target_id: 'target',
    tab_id: 'tab',
    refs: [
      { role: 'textbox', name: 'verification value', ref: 'p1:0', value },
      { role: 'button', name: 'Submit', ref: 'p1:1' },
    ],
  };
}

test('mock types before submitting', () => {
  const candidates = buildCandidates(snapshot(), 'expected');
  assert.equal(chooseMock(candidates).choice, 'type-verification-value');
  assert.equal(candidates[0].arguments.ref, 'p1:0');
});

test('mock submits once the value matches', () => {
  const candidates = buildCandidates(snapshot('expected'), 'expected');
  assert.equal(chooseMock(candidates).choice, 'submit-form');
});

test('unknown or stale choices fail closed', () => {
  assert.throws(() => validateChoice('stale-action', buildCandidates(snapshot(), 'expected')));
});

test('selected id resolves to the original immutable candidate arguments', () => {
  const candidates = buildCandidates(snapshot(), 'expected');
  const selected = validateChoice('type-verification-value', candidates);
  assert.equal(selected, candidates[0]);
  assert.deepEqual(selected.arguments, {
    target_id: 'target',
    tab_id: 'tab',
    ref: 'p1:0',
    text: 'expected',
    replace: true,
  });
});

test('only the external oracle can verify completion', () => {
  assert.equal(classify('expected', 'expected', 1, 4), 'verified');
  assert.equal(classify('wrong', 'expected', 1, 4), 'refuted');
  assert.equal(classify(null, 'expected', 4, 4), 'budget_exhausted');
});

test('live adapter sends one Choice keyed by executable candidate id', async () => {
  let requestBody: Record<string, any> | undefined;
  const client = new TypeSafeClient({
    apiKey: 'test-key',
    baseURL: 'http://provider.test',
    fetch: async (_input, init) => {
      requestBody = JSON.parse(String(init?.body));
      return new Response(
        JSON.stringify({
          model: 'jev-latest',
          usage: { input_tokens: 10, output_tokens: 2 },
          answers: {
            driver_action: {
              type: 'choice',
              choice: 'type-verification-value',
              confidence: 0.9,
              probabilities: { 'type-verification-value': 0.9 },
            },
          },
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      );
    },
  });
  const page = snapshot();
  const candidates = buildCandidates(page, 'expected');
  const answer = await chooseWithTypeSafe(client, candidates, page, []);

  assert.equal(answer.choice, 'type-verification-value');
  assert.deepEqual(
    new Set(Object.keys(requestBody?.questions.driver_action.criteria)),
    new Set(['type-verification-value', 'abstain'])
  );
});

test('driver repeats the explicit session label', async () => {
  const calls: unknown[] = [];
  const client = {
    callTool: async (request: unknown) => {
      calls.push(request);
      return { isError: false, structuredContent: { status: 'ok' } };
    },
  };
  const driver = new Driver(client as never, 'jev-test');
  await driver.call('browser_type', { ref: 'p2:0' });
  assert.deepEqual(calls, [
    { name: 'browser_type', arguments: { ref: 'p2:0', session: 'jev-test' } },
  ]);
});

test('fixture URL is confined to loopback HTTP', () => {
  assert.equal(validateFixtureUrl('http://127.0.0.1:8765'), 'http://127.0.0.1:8765/');
  for (const value of ['https://127.0.0.1/', 'http://example.com/', 'http://localhost/api/']) {
    assert.throws(() => validateFixtureUrl(value));
  }
});

test('tab selection accepts unknown active state', () => {
  assert.equal(selectTabId([{ tab_id: 'first', active: null }]), 'first');
  assert.equal(
    selectTabId([
      { tab_id: 'first', active: false },
      { tab_id: 'second', active: true },
    ]),
    'second'
  );
  assert.throws(() => selectTabId([]));
});

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { TypeSafeClient } from '@typesafe-ai/sdk';

import {
  buildCandidates,
  chooseMock,
  classify,
  parseVisualRegions,
  validateChoice,
  type Candidate,
} from './core.js';
import {
  chooseWithTypeSafe,
  Driver,
  optionalVisualObservation,
  selectTabId,
  validateFixtureUrl,
} from './run.js';

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

function fixture(name: string): any {
  return JSON.parse(readFileSync(new URL(`../fixtures/${name}`, import.meta.url), 'utf8'));
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
  assert.throws(() => Object.assign(selected.arguments, { ref: 'changed' }));
  assert.throws(() => Object.assign(selected, { captureId: 'changed' }));
});

test('reserved candidates are always available', () => {
  const candidates = buildCandidates({ target_id: 'target', tab_id: 'tab', refs: [] }, 'expected');
  assert.deepEqual(
    candidates.map((candidate) => candidate.id),
    ['reobserve', 'abstain']
  );
  assert.equal(chooseMock(candidates).choice, 'reobserve');
});

test('visual fixture builds the equivalent immutable capture-bound submit candidate', () => {
  const visual = parseVisualRegions(
    fixture('parse-visual-regions-submit-v1.json'),
    'capture-submit',
    'target',
    'tab'
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  const selected = validateChoice(
    'submit-form',
    buildCandidates(page, 'expected', visual),
    'capture-submit'
  );
  assert.equal(selected.id, 'submit-form');
  assert.equal(selected.captureId, 'capture-submit');
  assert.equal(selected.screenshotReference, 'png-sha256:submit-fixture');
  assert.deepEqual(selected.arguments, { target_id: 'target', tab_id: 'tab', x: 275, y: 720 });
});

test('ambiguous visual regions offer only reobserve and abstain', () => {
  const visual = parseVisualRegions(
    fixture('parse-visual-regions-ambiguous-v1.json'),
    'capture-ambiguous',
    'target',
    'tab'
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  assert.deepEqual(
    buildCandidates(page, 'expected', visual).map((candidate) => candidate.id),
    ['reobserve', 'abstain']
  );
});

test('stale, malformed, and duplicate visual selections fail closed', () => {
  const payload = fixture('parse-visual-regions-submit-v1.json');
  assert.throws(
    () => parseVisualRegions(payload, 'new-capture', 'target', 'tab'),
    /stale/
  );
  payload.regions[0].bounds.width = 900;
  assert.throws(
    () => parseVisualRegions(payload, 'capture-submit', 'target', 'tab'),
    /outside/
  );
  const duplicate: Candidate = Object.freeze({
    id: 'duplicate',
    description: 'duplicate',
    tool: null,
    arguments: Object.freeze({}),
  });
  assert.throws(() => validateChoice('duplicate', [duplicate, duplicate]), /duplicate/);
});

test('capture-bound selection rejects a newer capture', () => {
  const visual = parseVisualRegions(
    fixture('parse-visual-regions-submit-v1.json'),
    'capture-submit',
    'target',
    'tab'
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  assert.throws(
    () => validateChoice('submit-form', buildCandidates(page, 'expected', visual), 'new-capture'),
    /stale/
  );
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
    new Set(['type-verification-value', 'reobserve', 'abstain'])
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

test('visual tool is optional and uses the released contract when advertised', async () => {
  const calls: any[] = [];
  const client = {
    callTool: async (request: unknown) => {
      calls.push(request);
      return {
        isError: false,
        structuredContent: fixture('parse-visual-regions-submit-v1.json'),
      };
    },
  };
  const driver = new Driver(client as never, 'jev-test');
  const page = { target_id: 'target', tab_id: 'tab', capture_id: 'capture-submit' };
  assert.equal(await optionalVisualObservation(driver, page, new Set()), undefined);
  const visual = await optionalVisualObservation(
    driver,
    page,
    new Set(['parse_visual_regions'])
  );
  assert.equal(visual?.captureId, 'capture-submit');
  assert.deepEqual(calls, [
    {
      name: 'parse_visual_regions',
      arguments: {
        capture_id: 'capture-submit',
        options: { kinds: ['text', 'icon'], min_confidence: 0.8, max_regions: 100 },
        session: 'jev-test',
      },
    },
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

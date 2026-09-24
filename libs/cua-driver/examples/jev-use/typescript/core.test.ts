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
  Driver,
  optionalVisualObservation,
  selectTabId,
  supportsCaptureBoundClick,
  validateFixtureUrl,
} from './run.js';
import { chooseWithTypeSafe } from './jev_adapter.js';

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

test('visual fixture builds a candidate without claiming interactivity', () => {
  const payload = fixture('parse-visual-regions-submit-v1.json');
  payload.regions[0].interactive = false;
  const visual = parseVisualRegions(
    payload,
    'capture-submit',
    7,
    9
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  const selected = validateChoice(
    'submit-form',
    buildCandidates(page, 'expected', visual, true),
    'capture-submit'
  );
  assert.equal(selected.id, 'submit-form');
  assert.equal(selected.captureId, 'capture-submit');
  assert.equal(selected.screenshotReference, 'png-sha256:submit-fixture');
  assert.deepEqual(selected.arguments, {
    pid: 7,
    window_id: 9,
    x: 350,
    y: 260,
    capture_id: 'capture-submit',
    delivery_mode: 'background',
  });
});

test('ambiguous visual regions offer only reobserve and abstain', () => {
  const payload = fixture('parse-visual-regions-ambiguous-v1.json');
  for (const region of payload.regions) region.interactive = false;
  const visual = parseVisualRegions(
    payload,
    'capture-ambiguous',
    7,
    9
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  assert.deepEqual(
    buildCandidates(page, 'expected', visual, true).map((candidate) => candidate.id),
    ['reobserve', 'abstain']
  );
});

test('non-Submit visual observation offers only reobserve and abstain', () => {
  const payload = fixture('parse-visual-regions-submit-v1.json');
  payload.regions[0].text = 'Continue';
  payload.regions[0].interactive = true;
  const visual = parseVisualRegions(payload, 'capture-submit', 7, 9);
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  assert.deepEqual(
    buildCandidates(page, 'expected', visual, true).map((candidate) => candidate.id),
    ['reobserve', 'abstain']
  );
});

test('visual candidate requires the capture-bound click contract', () => {
  const visual = parseVisualRegions(
    fixture('parse-visual-regions-submit-v1.json'),
    'capture-submit',
    7,
    9
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  assert.deepEqual(
    buildCandidates(page, 'expected', visual).map((candidate) => candidate.id),
    ['reobserve', 'abstain']
  );
});

test('null optionals and ASCII case rules match Python', () => {
  const visual = parseVisualRegions(
    fixture('parse-visual-regions-null-and-case-v1.json'),
    'capture-edge',
    7,
    9
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  const selected = validateChoice(
    'submit-form',
    buildCandidates(page, 'expected', visual, true),
    'capture-edge'
  );
  assert.equal(selected.arguments.x, 140);
  assert.equal(selected.arguments.capture_id, 'capture-edge');
});

test('stale, malformed, and duplicate visual selections fail closed', () => {
  const payload = fixture('parse-visual-regions-submit-v1.json');
  assert.throws(
    () => parseVisualRegions(payload, 'new-capture', 7, 9),
    /stale/
  );
  payload.regions[0].bounds.width = 900;
  assert.throws(
    () => parseVisualRegions(payload, 'capture-submit', 7, 9),
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
    7,
    9
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  assert.throws(
    () => validateChoice('submit-form', buildCandidates(page, 'expected', visual, true), 'new-capture'),
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
              choice: 'submit-form',
              confidence: 0.9,
              probabilities: { 'submit-form': 0.9 },
            },
          },
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      );
    },
  });
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  const visual = parseVisualRegions(
    fixture('parse-visual-regions-submit-v1.json'),
    'capture-submit',
    7,
    9
  );
  const candidates = buildCandidates(page, 'expected', visual, true);
  const answer = await chooseWithTypeSafe(client, candidates, page, visual, []);

  assert.equal(answer.choice, 'submit-form');
  assert.deepEqual(
    new Set(Object.keys(requestBody?.questions.driver_action.criteria)),
    new Set(['submit-form', 'reobserve', 'abstain'])
  );
  const sentVisual = JSON.parse(requestBody?.state.observation.visual);
  assert.equal(sentVisual.capture_id, 'capture-submit');
  assert.equal(sentVisual.regions[0].id, 'submit-text');
});

test('live adapter rejects an id outside the supplied table', async () => {
  const client = {
    systemOne: async () => ({
      answers: {
        driver_action: {
          type: 'choice' as const,
          choice: 'invented',
          confidence: 1,
          probabilities: { invented: 1 },
        },
      },
    }),
  };
  const page = snapshot();
  const candidates = buildCandidates(page, 'expected');
  await assert.rejects(
    () => chooseWithTypeSafe(client as never, candidates, page, undefined, []),
    /unknown candidate/
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

test('visual tool is optional and uses the public contract when advertised', async () => {
  const calls: any[] = [];
  const responses = [
    { capture_id: 'capture-submit' },
    fixture('parse-visual-regions-submit-v1.json'),
  ];
  const client = {
    callTool: async (request: unknown) => {
      calls.push(request);
      return {
        isError: false,
        structuredContent: responses.shift(),
      };
    },
  };
  const driver = new Driver(client as never, 'jev-test');
  assert.equal(
    await optionalVisualObservation(
      driver,
      7,
      9,
      new Set(['get_window_state', 'parse_visual_regions', 'click']),
      false
    ),
    undefined
  );
  const visual = await optionalVisualObservation(
    driver,
    7,
    9,
    new Set(['get_window_state', 'parse_visual_regions', 'click']),
    true
  );
  assert.equal(visual?.captureId, 'capture-submit');
  assert.deepEqual(calls, [
    {
      name: 'get_window_state',
      arguments: {
        pid: 7,
        window_id: 9,
        include_accessibility_tree: false,
        session: 'jev-test',
      },
    },
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

test('capture-bound click requires capture_id in the advertised schema', () => {
  assert.equal(
    supportsCaptureBoundClick([{ name: 'click', inputSchema: { properties: { x: {} } } }]),
    false
  );
  assert.equal(
    supportsCaptureBoundClick([
      { name: 'click', inputSchema: { properties: { capture_id: {} } } },
    ]),
    true
  );
});

test('provider delay then capture change refuses without an unbound retry', async () => {
  const calls: any[] = [];
  const client = {
    callTool: async (request: any) => {
      calls.push(request);
      if (request.name === 'get_window_state') {
        return { isError: false, structuredContent: { capture_id: 'capture-submit' } };
      }
      if (request.name === 'parse_visual_regions') {
        return {
          isError: false,
          structuredContent: fixture('parse-visual-regions-submit-v1.json'),
        };
      }
      if (request.name === 'click') {
        return {
          isError: true,
          structuredContent: { code: 'capture_generation_mismatch' },
          content: [{ type: 'text', text: 'capture changed while provider was deciding' }],
        };
      }
      throw new Error(request.name);
    },
  };
  const driver = new Driver(client as never, 'jev-test');
  const visual = await optionalVisualObservation(
    driver,
    7,
    9,
    new Set(['get_window_state', 'parse_visual_regions', 'click']),
    true
  );
  const page = snapshot('expected');
  page.refs = page.refs.slice(0, 1);
  const candidate = validateChoice(
    'submit-form',
    buildCandidates(page, 'expected', visual, true),
    'capture-submit'
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  await assert.rejects(() => driver.call(candidate.tool!, candidate.arguments), /click failed/);
  const clickCalls = calls.filter((call) => call.name === 'click');
  assert.equal(clickCalls.length, 1);
  assert.equal(clickCalls[0].arguments.capture_id, 'capture-submit');
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

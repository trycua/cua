import assert from 'node:assert/strict';
import test from 'node:test';

import { BACKENDS, browserDecisionRequest, isBrowserProvider } from './browser_provider.js';
import type { BrowserSnapshot } from './core.js';
import { FixtureFormTask, fixtureSources } from './tasks.js';

const TOKEN = 'secret-proof-token';

function fixture(captureId?: string) {
  const snapshot: BrowserSnapshot = {
    target_id: 'target',
    tab_id: 'tab',
    ...(captureId ? { capture_id: captureId } : {}),
    refs: [
      { role: 'textbox', name: 'verification value', ref: 'p1:0', value: '' },
      { role: 'button', name: 'Submit', ref: 'p1:1' },
    ],
  };
  const task = new FixtureFormTask(TOKEN);
  const sources = fixtureSources(snapshot);
  return { task, sources, candidates: task.candidates(sources) };
}

test('browser request is bounded and carries no action arguments or secret', () => {
  const { task, sources, candidates } = fixture('browser-capture-1');
  const request = browserDecisionRequest(task, sources, candidates, [
    task.historyEntry(1, 'reobserve'),
  ]);
  assert.equal(request.schema, 'cua.jev_choice_request_v1');
  assert.equal(request.capture_id, 'browser-capture-1');
  assert.deepEqual(
    request.candidates.map(({ id }) => id),
    ['type-verification-value', 'reobserve', 'abstain']
  );
  assert.deepEqual(request.history, [
    { selected_id: 'reobserve', outcome: task.historyEntry(1, 'reobserve').outcome },
  ]);
  const wire = JSON.stringify(request);
  for (const forbidden of [TOKEN, 'arguments', 'browser_type', 'p1:0']) {
    assert.equal(wire.includes(forbidden), false, forbidden);
  }
});

test('browser request falls back to the browser tab for its capture ID', () => {
  const { task, sources, candidates } = fixture();
  assert.equal(
    browserDecisionRequest(task, sources, candidates, []).capture_id,
    'browser:target:tab'
  );
});

test('each browser provider names its backend', () => {
  assert.deepEqual(BACKENDS, { mock: 'mock', live: 'typesafe', s1: 's1' });
  assert.equal(isBrowserProvider('s1'), true);
  assert.equal(isBrowserProvider('typesafe'), false);
  assert.equal(isBrowserProvider('toString'), false);
});

import assert from 'node:assert/strict';
import test, { mock } from 'node:test';

import {
  backendName,
  browserDecisionRequest,
  chooseBrowserProvider,
} from './browser_provider.js';
import { TypeSafeClient } from '@typesafe-ai/sdk';

import { FixtureFormTask, fixtureSources } from './tasks.js';
import type { BrowserSnapshot } from './core.js';

function fixture(token: string) {
  const snapshot: BrowserSnapshot = {
    target_id: 'target',
    tab_id: 'tab',
    capture_id: 'browser-capture-1',
    refs: [
      { role: 'textbox', name: 'verification value', ref: 'p1:0', value: '' },
      { role: 'button', name: 'Submit', ref: 'p1:1' },
    ],
  };
  const task = new FixtureFormTask(token);
  const sources = fixtureSources(snapshot);
  return { task, sources, candidates: task.candidates(sources) };
}

test('browser request is bounded and contains no action arguments or secret', () => {
  const token = 'secret-proof-token';
  const { task, sources, candidates } = fixture(token);
  const request = browserDecisionRequest(task, sources, candidates, []);
  assert.equal(request.schema, 'cua.jev_choice_request_v1');
  assert.equal(request.capture_id, 'browser-capture-1');
  assert.deepEqual(
    request.candidates.map(({ id }) => id),
    ['type-verification-value', 'reobserve', 'abstain']
  );
  const wire = JSON.stringify(request);
  assert.equal(wire.includes(token), false);
  assert.equal(wire.includes('arguments'), false);
  assert.equal(wire.includes('browser_type'), false);
});

test('mock keeps existing task policy and reports actual backend', async () => {
  const { task, sources, candidates } = fixture('proof-token');
  const result = await chooseBrowserProvider('mock', task, sources, candidates, []);
  assert.equal(result.choice, 'type-verification-value');
  assert.equal(result.confidence, 1);
  assert.equal(result.backend, 'mock');
  assert.equal(result.probabilities[result.choice!], 1);
  assert.equal(backendName('live'), 'typesafe');
  assert.equal(backendName('typesafe'), 'typesafe');
  assert.equal(backendName('s1'), 's1');
});

test('S1 uses the bounded browser request and reports s1 as the actual backend', async () => {
  const { task, sources, candidates } = fixture('proof-token');
  mock.method(globalThis, 'fetch', async (_input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
    const body = JSON.parse(String(init?.body));
    assert.equal(body.schema, 'cua.jev_choice_request_v1');
    assert.equal(JSON.stringify(body).includes('arguments'), false);
    return new Response(
      JSON.stringify({
        selected_id: 'type-verification-value',
        confidence: 0.7,
        probabilities: {
          'type-verification-value': 0.7,
          reobserve: 0.2,
          abstain: 0.1,
        },
      }),
      { status: 200 }
    );
  });
  const result = await chooseBrowserProvider('s1', task, sources, candidates, []);
  assert.equal(result.choice, 'type-verification-value');
  assert.equal(result.backend, 's1');
  assert.equal(result.probabilities['type-verification-value'], 0.7);
  mock.restoreAll();
});

test('live and typesafe aliases report typesafe after a bounded decision', async () => {
  const { task, sources, candidates } = fixture('proof-token');
  mock.method(TypeSafeClient.prototype, 'systemOne', async () => ({
    answers: {
      candidate: {
        type: 'choice',
        choice: 'type-verification-value',
        confidence: 0.8,
        probabilities: {
          'type-verification-value': 0.8,
          reobserve: 0.1,
          abstain: 0.1,
        },
      },
    },
  }));
  for (const provider of ['live', 'typesafe'] as const) {
    const result = await chooseBrowserProvider(provider, task, sources, candidates, []);
    assert.equal(result.choice, 'type-verification-value');
    assert.equal(result.backend, 'typesafe');
    assert.equal(result.probabilities['type-verification-value'], 0.8);
  }
  mock.restoreAll();
});
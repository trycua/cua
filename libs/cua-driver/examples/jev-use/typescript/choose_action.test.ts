import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { TypeSafeClient } from '@typesafe-ai/sdk';

import { chooseRequest, validateRequest } from './choose_action.js';

function request(): any {
  return JSON.parse(
    readFileSync(new URL('../fixtures/jev-choice-request-v1.json', import.meta.url), 'utf8')
  );
}

test('bounded chooser mock emits only the fixed response', async () => {
  const response = await chooseRequest(request(), { mock: true });
  assert.deepEqual(
    new Set(Object.keys(response)),
    new Set(['schema', 'selected_id', 'model', 'confidence', 'probabilities'])
  );
  assert.equal(response.schema, 'cua.jev_choice_v1');
  assert.equal(response.selected_id, 'submit-form');
  assert.equal(response.model, 'mock');
});

test('bounded chooser uses the SDK and preserves provider model identity', async () => {
  let sent: Record<string, any> | undefined;
  const client = new TypeSafeClient({
    apiKey: 'test-key',
    baseURL: 'http://provider.test',
    fetch: async (_input, init) => {
      sent = JSON.parse(String(init?.body));
      return new Response(
        JSON.stringify({
          model: 'jev-test',
          usage: { input_tokens: 10, output_tokens: 2 },
          answers: {
            candidate: {
              type: 'choice',
              choice: 'reobserve',
              confidence: 0.75,
              probabilities: { reobserve: 0.75, abstain: 0.25 },
            },
          },
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      );
    },
  });
  const response = await chooseRequest(request(), { client });
  assert.equal(response.selected_id, 'reobserve');
  assert.equal(response.model, 'jev-test');
  assert.deepEqual(
    new Set(Object.keys(sent?.questions.candidate.criteria)),
    new Set(['submit-form', 'reobserve', 'abstain'])
  );
  assert.equal(sent?.questions.candidate.instructions, request().goal);
  assert.deepEqual(Object.keys(sent?.state ?? {}), ['observation']);
  const observation = JSON.parse(sent?.state.observation);
  assert.equal(observation.capture_id, 'capture-fixture-1');
  assert.equal(observation.regions[0].id, 'submit-text');
  assert.equal(JSON.stringify(sent).includes('arguments'), false);
});

test('bounded chooser rejects unsafe history and missing reserved IDs', () => {
  for (const field of ['arguments', 'screenshot', 'image_base64']) {
    const invalid = request();
    invalid.history = [{ [field]: 'secret' }];
    assert.throws(() => validateRequest(invalid), /forbidden/);
  }
  const invalid = request();
  invalid.candidates = invalid.candidates.slice(0, 2);
  assert.throws(() => validateRequest(invalid), /reobserve and abstain/);
});

test('bounded chooser rejects a provider ID outside the request', async () => {
  const client = {
    systemOne: async () => ({
      model: 'jev-test',
      answers: {
        candidate: {
          type: 'choice' as const,
          choice: 'invented',
          confidence: 1,
          probabilities: { invented: 1 },
        },
      },
    }),
  };
  await assert.rejects(
    () => chooseRequest(request(), { client: client as never }),
    /unknown candidate/
  );
});

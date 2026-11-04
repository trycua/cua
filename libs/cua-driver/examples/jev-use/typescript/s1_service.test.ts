import assert from 'node:assert/strict';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import test from 'node:test';

import { parseArgs } from './run_native.js';
import { chooseS1Service, S1ServiceError, s1ServiceUrl, validateDecision } from './s1_service.js';

const REQUEST = {
  schema: 'cua.jev_choice_request_v2',
  goal: 'Set the counter to 3.',
  capture_id: 'cap-1',
  regions: [],
  history: [],
  candidates: [
    { id: 'ax:button:increment', description: 'Click Increment', source: 'ax' },
    { id: 'reobserve', description: 'Observe again' },
    { id: 'abstain', description: 'Stop' },
  ],
};

function decision(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    schema: 'cua.decision_choice_v1',
    kind: 'selected',
    capture_id: 'cap-1',
    selected_id: 'ax:button:increment',
    model: 'cua-s1-4b-0.2',
    confidence: 0.9,
    probabilities: { 'ax:button:increment': 0.9, reobserve: 0.06, abstain: 0.04 },
    reason: null,
    ...overrides,
  };
}

async function serve(status: number, body: string, received: unknown[]): Promise<{ server: Server; url: string }> {
  const server = createServer((request, response) => {
    let data = '';
    request.on('data', (chunk) => { data += chunk; });
    request.on('end', () => {
      received.push(JSON.parse(data));
      response.writeHead(status, { 'Content-Type': 'application/json' });
      response.end(body);
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return { server, url: `http://127.0.0.1:${port}/decide` };
}

test('s1ServiceUrl accepts only loopback http', () => {
  for (const url of ['http://127.0.0.1:8791/decide', 'http://localhost:8791/decide', 'http://[::1]:8791/decide']) {
    assert.equal(s1ServiceUrl(url), url);
  }
  for (const url of [
    'http://10.0.0.5:8791/decide',
    'http://example.com/decide',
    'https://127.0.0.1:8791/decide',
    'http://user:secret@127.0.0.1:8791/decide',
    '',
  ]) {
    assert.throws(() => s1ServiceUrl(url), S1ServiceError, url);
  }
});

test('validateDecision accepts selected, reobserve, and abstain', () => {
  assert.equal(validateDecision(decision(), REQUEST).selected_id, 'ax:button:increment');
  for (const choice of ['reobserve', 'abstain']) {
    const probabilities = { 'ax:button:increment': 0.1, reobserve: 0.1, abstain: 0.1, [choice]: 0.8 };
    assert.equal(
      validateDecision(decision({ kind: choice, selected_id: choice, confidence: 0.8, probabilities }), REQUEST).kind,
      choice,
    );
  }
});

test('validateDecision rejects malformed decisions', () => {
  const bad: unknown[] = [
    decision({ schema: 'cua.decision_choice_v0' }),
    decision({ kind: 'act' }),
    decision({ capture_id: 'cap-2' }),
    decision({ kind: 'error', selected_id: null, reason: 'model_error' }),
    decision({ selected_id: 'ax:button:reset' }),
    decision({ kind: 'reobserve' }),
    decision({ probabilities: { 'ax:button:increment': 1 } }),
    decision({ probabilities: { 'ax:button:increment': 1.5, reobserve: 0, abstain: 0 } }),
    decision({ confidence: Number.NaN }),
    decision({ confidence: true }),
    [],
  ];
  for (const value of bad) assert.throws(() => validateDecision(value, REQUEST), S1ServiceError);
});

test('chooseS1Service posts the request and returns the choice', async () => {
  const received: unknown[] = [];
  const { server, url } = await serve(200, JSON.stringify(decision()), received);
  try {
    const result = await chooseS1Service(REQUEST, url);
    assert.deepEqual(received, [REQUEST]);
    assert.equal(result.choice, 'ax:button:increment');
    assert.equal(result.confidence, 0.9);
    assert.deepEqual(Object.keys(result.probabilities).sort(), ['abstain', 'ax:button:increment', 'reobserve']);
  } finally {
    server.close();
  }
});

test('chooseS1Service rejects HTTP errors, invalid JSON, and an unreachable service', async () => {
  for (const [status, body] of [[400, '{"error":"invalid_request"}'], [200, 'not json']] as const) {
    const { server, url } = await serve(status, body, []);
    try {
      await assert.rejects(chooseS1Service(REQUEST, url), S1ServiceError);
    } finally {
      server.close();
    }
  }
  const { server, url } = await serve(200, '{}', []);
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await assert.rejects(chooseS1Service(REQUEST, url, 2_000), S1ServiceError);
});

test('the native runner accepts the s1 provider', () => {
  const args = parseArgs(['--task', 'appkit-counter', '--provider', 's1', '--pid', '1', '--state-file', 's']);
  assert.equal(args.provider, 's1');
});

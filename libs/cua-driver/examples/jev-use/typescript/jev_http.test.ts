/** Real loopback HTTP coverage; no model, provider credentials, or fetch mock. */
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createServer, type IncomingHttpHeaders } from 'node:http';
import test, { type TestContext } from 'node:test';

import { chooseWithBackend, readJevConfig } from './jev_backends.js';

const CRITERIA = {
  'type-value': 'Enter the value.',
  reobserve: 'Observe again.',
};

function answer(choice = 'type-value') {
  return {
    model: 'loopback-fixture',
    answers: {
      candidate: {
        type: 'choice',
        choice,
        confidence: choice === 'type-value' ? 0.8 : 0.2,
        probabilities: { 'type-value': 0.8, reobserve: 0.2 },
      },
    },
  };
}

type Received = {
  path: string | undefined;
  headers: IncomingHttpHeaders;
  payload: unknown;
};

type FixtureOptions = {
  status?: number;
  stall?: 'before-headers' | 'body';
};

async function fixture(t: TestContext, payload: unknown, options: FixtureOptions = {}) {
  const received: Received[] = [];
  let bodyStarted = false;
  const body = typeof payload === 'string' ? payload : JSON.stringify(payload);
  const server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on('data', (chunk: Buffer) => chunks.push(chunk));
    request.on('end', () => {
      received.push({
        path: request.url,
        headers: request.headers,
        payload: JSON.parse(Buffer.concat(chunks).toString('utf8')),
      });
      if (options.stall === 'before-headers') return;
      response.writeHead(options.status ?? 200, { 'Content-Type': 'application/json' });
      if (options.stall === 'body') {
        // A valid JSON prefix, not a malformed completed response. The real
        // client must hit its deadline while awaiting the remaining bytes.
        response.flushHeaders();
        response.write('{"model":');
        bodyStarted = true;
        return;
      }
      response.end(body);
    });
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(async () => {
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()));
    });
  });
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  return {
    url: `http://127.0.0.1:${address.port}`,
    received,
    bodyStarted: () => bodyStarted,
  };
}

function call(url: string, timeoutMs = 1000) {
  // An explicit dictionary avoids inheriting real provider credentials.
  return chooseWithBackend(
    readJevConfig({
      JEV_BACKEND: 'local',
      JEV_BASE_URL: url,
      JEV_MODEL: 'fixture-model',
      JEV_TIMEOUT_MS: String(timeoutMs),
    }),
    {
      goal: 'Enter a value.',
      observation: { label: 'café 🙂' },
      criteria: CRITERIA,
    }
  );
}

test('real HTTP success preserves POST payload, Unicode, and candidate IDs', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, answer());
  const outcome = await call(f.url);
  assert.equal(outcome.ok, true);
  assert.equal(outcome.decision?.selectedId, 'type-value');
  assert.equal(outcome.decision?.model, 'loopback-fixture');
  assert.equal(f.received.length, 1);
  assert.equal(f.received[0].path, '/v1/systemone');
  assert.equal(f.received[0].headers.authorization, undefined);
  assert.equal(f.received[0].headers['content-type'], 'application/json');
  assert.deepEqual(f.received[0].payload, {
    state: { goal: 'Enter a value.', observation: { label: 'café 🙂' } },
    model: 'fixture-model',
    questions: {
      candidate: {
        type: 'choice',
        instructions: 'Select exactly one supplied candidate ID.',
        criteria: CRITERIA,
      },
    },
  });
});

test('real HTTP error returns a skip without reposting', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, { error: 'fixture unavailable' }, { status: 503 });
  const outcome = await call(f.url);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'http_error');
  assert.equal(outcome.decision, undefined);
  assert.equal(f.received.length, 1);
});

test('completed non-JSON body remains invalid_response', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, 'not JSON');
  const outcome = await call(f.url);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'invalid_response');
  assert.equal(outcome.decision, undefined);
  assert.equal(f.received.length, 1);
});

test('real HTTP missing answers returns invalid_response', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, { model: 'fixture' });
  const outcome = await call(f.url);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'invalid_response');
  assert.equal(outcome.decision, undefined);
  assert.equal(f.received.length, 1);
});

test('real HTTP valid mass with wrong winner is rejected', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, answer('reobserve'));
  const outcome = await call(f.url);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'invalid_response');
  assert.match(outcome.message ?? '', /argmax/);
  assert.equal(outcome.decision, undefined);
  assert.equal(f.received.length, 1);
});

test('timeout before headers returns timeout without reposting', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, answer(), { stall: 'before-headers' });
  const outcome = await call(f.url, 250);
  assert.equal(f.received.length, 1, 'fixture received the request before timeout');
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'timeout');
  assert.equal(outcome.decision, undefined);
});

test('timeout while reading a successful response body stays timeout', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, answer(), { stall: 'body' });
  const outcome = await call(f.url, 250);
  assert.equal(f.received.length, 1);
  assert.equal(f.bodyStarted(), true, 'fixture sent headers and an incomplete body');
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'timeout');
  assert.equal(outcome.decision, undefined);
});

test('known HTTP error status survives a stalled diagnostic body', { timeout: 5000 }, async (t) => {
  const f = await fixture(t, {}, { status: 503, stall: 'body' });
  const outcome = await call(f.url, 250);
  assert.equal(f.bodyStarted(), true);
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'http_error');
  assert.match(outcome.message ?? '', /HTTP 503/);
  assert.equal(outcome.decision, undefined);
  assert.equal(f.received.length, 1);
});

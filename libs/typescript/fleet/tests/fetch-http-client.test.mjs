import assert from 'node:assert/strict';
import test from 'node:test';

import {
  RESPONSE_LIMIT_ERROR,
  composeAbortSignals,
  createTimeoutSignal,
  executeFetchRequest,
} from '../scripts/build.mjs';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function request(overrides = {}) {
  return {
    method: 'GET',
    url: 'https://example.invalid/resource',
    headers: [],
    ...overrides,
  };
}

function chunkedResponse(chunks, headers = {}) {
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(encoder.encode(chunk));
        }
        controller.close();
      },
    }),
    { status: 200, headers },
  );
}

function fetchReturning(response) {
  return async () => response;
}

function fetchUntilAborted(_url, { signal }) {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    signal?.addEventListener('abort', () => reject(signal.reason), { once: true });
  });
}

test('accepts an exact-size chunked body without Content-Length', async () => {
  const result = await executeFetchRequest(
    request({ maxResponseBytes: 6n }),
    undefined,
    fetchReturning(chunkedResponse(['ab', 'cdef'])),
  );

  assert.equal(decoder.decode(result.body), 'abcdef');
});

test('rejects a chunked body as soon as it exceeds the limit', async () => {
  await assert.rejects(
    executeFetchRequest(
      request({ maxResponseBytes: 5n }),
      undefined,
      fetchReturning(chunkedResponse(['abc', 'def'])),
    ),
    { message: RESPONSE_LIMIT_ERROR },
  );
});

test('uses streamed bytes rather than a lying Content-Length', async () => {
  const response = chunkedResponse(['ab', 'cd'], { 'content-length': '1' });

  await assert.rejects(
    executeFetchRequest(
      request({ maxResponseBytes: 3n }),
      undefined,
      fetchReturning(response),
    ),
    { message: RESPONSE_LIMIT_ERROR },
  );
});

test('leaves response size unlimited when maxResponseBytes is omitted', async () => {
  const result = await executeFetchRequest(
    request(),
    undefined,
    fetchReturning(chunkedResponse(['un', 'limited'])),
  );

  assert.equal(decoder.decode(result.body), 'unlimited');
});

test('treats maxResponseBytes zero as an empty-body-only limit', async () => {
  const empty = await executeFetchRequest(
    request({ maxResponseBytes: 0n }),
    undefined,
    fetchReturning(chunkedResponse([])),
  );
  assert.equal(empty.body.byteLength, 0);

  await assert.rejects(
    executeFetchRequest(
      request({ maxResponseBytes: 0n }),
      undefined,
      fetchReturning(chunkedResponse(['x'])),
    ),
    { message: RESPONSE_LIMIT_ERROR },
  );
});

test('cancels the reader and does not leak response details on overflow', async () => {
  const secret = 'customer-token-and-body';
  let cancelled = false;
  const response = new Response(
    new ReadableStream({
      start(controller) {
        controller.enqueue(encoder.encode(secret));
      },
      cancel() {
        cancelled = true;
        throw new Error(`cancel failed: ${secret}`);
      },
    }),
  );

  const error = await executeFetchRequest(
    request({
      url: `https://example.invalid/resource?token=${secret}`,
      maxResponseBytes: 1n,
    }),
    undefined,
    fetchReturning(response),
  ).then(
    () => assert.fail('expected the response limit to reject'),
    (caught) => caught,
  );

  assert.equal(cancelled, true);
  assert.equal(error.message, RESPONSE_LIMIT_ERROR);
  assert.equal(error.message.includes(secret), false);
});

test('caller cancellation remains effective when a timeout is configured', async () => {
  const controller = new AbortController();
  const pending = executeFetchRequest(
    request({ timeoutSecs: 10n }),
    controller.signal,
    fetchUntilAborted,
  );
  controller.abort(new DOMException('caller cancelled', 'AbortError'));

  await assert.rejects(pending, { name: 'AbortError' });
});

test('caller cancellation remains effective while the response body is streaming', async () => {
  const controller = new AbortController();
  const streamingFetch = async (_url, { signal }) =>
    new Response(
      new ReadableStream({
        start(bodyController) {
          bodyController.enqueue(encoder.encode('partial'));
          signal.addEventListener(
            'abort',
            () => bodyController.error(signal.reason),
            { once: true },
          );
        },
      }),
    );
  const pending = executeFetchRequest(
    request({ timeoutSecs: 10n }),
    controller.signal,
    streamingFetch,
  );
  controller.abort(new DOMException('caller cancelled', 'AbortError'));

  await assert.rejects(pending, { name: 'AbortError' });
});

test('configured timeout aborts a pending request', async () => {
  await assert.rejects(
    executeFetchRequest(request({ timeoutSecs: 1n }), undefined, fetchUntilAborted),
    { name: 'TimeoutError' },
  );
});

test('timeoutSecs zero is an explicit immediate timeout', async () => {
  await assert.rejects(
    executeFetchRequest(request({ timeoutSecs: 0n }), undefined, fetchUntilAborted),
    { name: 'TimeoutError' },
  );
});

test('signal composition fallback preserves either abort source', () => {
  const caller = new AbortController();
  const timeout = new AbortController();
  const combined = composeAbortSignals([caller.signal, timeout.signal], null);

  timeout.abort(new DOMException('timed out', 'TimeoutError'));
  assert.equal(combined.signal.aborted, true);
  assert.equal(combined.signal.reason.name, 'TimeoutError');
  combined.dispose();
});

test('timeout fallback aborts without AbortSignal.timeout', async () => {
  const timeout = createTimeoutSignal(1, null);
  await new Promise((resolve) => timeout.signal.addEventListener('abort', resolve, { once: true }));

  assert.equal(timeout.signal.reason.name, 'TimeoutError');
  timeout.dispose();
});

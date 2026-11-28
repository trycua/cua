import assert from 'node:assert/strict';
import { createServer, type RequestListener } from 'node:http';
import type { AddressInfo } from 'node:net';
import { test, type TestContext } from 'node:test';

import { resetFixture, waitForSubmission } from './fixture-client.js';

async function withServer(
  t: TestContext,
  handler: RequestListener,
  run: (url: string) => Promise<void>
) {
  const server = createServer(handler);
  t.after(async () => {
    const closed = new Promise<void>((resolve, reject) =>
      server.close((error) => error ? reject(error) : resolve())
    );
    server.closeAllConnections();
    await closed;
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  await run(`http://127.0.0.1:${(server.address() as AddressInfo).port}`);
}

for (const phase of ['headers', 'body'] as const) {
  for (const operation of ['reset', 'submission'] as const) {
    test(`${operation} aborts a stalled ${phase}`, { timeout: 5000 }, async (t) => {
      let requested = false;
      let resolveClosed!: () => void;
      const socketClosed = new Promise<void>((resolve) => { resolveClosed = resolve; });
      await withServer(t, (request, response) => {
        requested = true;
        request.socket.once('close', resolveClosed);
        if (phase === 'body') {
          response.writeHead(200, { 'Content-Type': 'application/json' });
          response.write('{');
        }
      }, async (url) => {
        const promise = operation === 'reset'
          ? resetFixture(url, 250)
          : waitForSubmission(url, 'expected', 250);
        await assert.rejects(promise, {
          message: operation === 'reset'
            ? 'fixture reset timed out'
            : 'fixture submission verification timed out',
        });
        assert.equal(requested, true);
        await socketClosed;
      });
    });
  }
}

test('reset uses POST and submission polls until the expected token arrives', async (t) => {
  let reads = 0;
  let resetMethod: string | undefined;
  await withServer(t, (request, response) => {
    if (request.url === '/reset') {
      resetMethod = request.method;
      response.writeHead(204).end();
    } else {
      reads++;
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify({ submitted: reads === 1 ? 'other' : 'expected' }));
    }
  }, async (url) => {
    await resetFixture(url, 2000);
    await waitForSubmission(url, 'expected', 2000);
  });
  assert.equal(resetMethod, 'POST');
  assert.equal(reads, 2);
});

test('submission shares one deadline across successful polls', { timeout: 5000 }, async (t) => {
  let reads = 0;
  await withServer(t, (_request, response) => {
    reads++;
    response.end('{"submitted":null}');
  }, async (url) => {
    await assert.rejects(waitForSubmission(url, 'expected', 350), {
      message: 'fixture submission verification timed out',
    });
  });
  assert.ok(reads >= 2);
});

for (const operation of ['reset', 'submission'] as const) {
  test(`${operation} preserves an HTTP error`, async (t) => {
    await withServer(t, (_request, response) => response.writeHead(503).end(), async (url) => {
      await assert.rejects(operation === 'reset'
        ? resetFixture(url, 2000)
        : waitForSubmission(url, 'expected', 2000), {
        message: operation === 'reset' ? 'fixture reset failed: 503' : 'fixture state failed: 503',
      });
    });
  });
}

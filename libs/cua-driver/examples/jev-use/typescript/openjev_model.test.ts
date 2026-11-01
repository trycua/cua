import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import assert from 'node:assert/strict';
import test from 'node:test';

import { REQUEST_SCHEMA_V2 } from './choose_action.js';
import {
  OpenJevDecisionModel,
  OpenJevError,
  openJevSystemOneUrl,
  readOpenJevConfig,
  validateOpenJevBaseUrl,
  type OpenJevTransport,
} from './openjev_model.js';

function request() {
  return {
    schema: REQUEST_SCHEMA_V2,
    goal: 'Submit the completed form.',
    capture_id: 'cap-1',
    snapshot_id: 'snap-1',
    regions: [],
    elements: [{ role_class: 'button', label: 'Submit', state: 'enabled' }],
    progress: [{ step: 'fill field', done: 1, required: 1 }],
    history: [{ selected_id: 'type-value' }],
    candidates: [
      { id: 'submit', description: 'Submit.', source: 'ax' },
      { id: 'reobserve', description: 'Observe again.' },
      { id: 'abstain', description: 'Stop.' },
    ],
  };
}

function goodResponse(): {
  model: string;
  answers: {
    candidate: {
      type: string;
      choice: string;
      confidence: number;
      probabilities: Record<string, number>;
    };
  };
} {
  return {
    model: 'openjev-fixture',
    answers: {
      candidate: {
        type: 'choice',
        choice: 'submit',
        confidence: 0.9,
        probabilities: { submit: 0.9, reobserve: 0.05, abstain: 0.05 },
      },
    },
  };
}

test('config is explicit and bounded', () => {
  const config = readOpenJevConfig({
    OPENJEV_BASE_URL: 'https://jev.example/v1/',
    OPENJEV_MODEL: 'custom',
    OPENJEV_TIMEOUT_MS: '999999',
  });
  assert.equal(config.baseUrl, 'https://jev.example/v1/');
  assert.equal(config.model, 'custom');
  assert.equal(config.timeoutMs, 60000);
});

test('API key requires HTTPS and URL credentials are rejected', () => {
  assert.throws(
    () => validateOpenJevBaseUrl('http://jev.example', true),
    /https/
  );
  assert.throws(
    () => validateOpenJevBaseUrl('https://user:pass@jev.example', false),
    /embed credentials/
  );
  for (const suffix of ['?tenant=a', '#fragment']) {
    assert.throws(
      () => validateOpenJevBaseUrl('https://jev.example' + suffix, false),
      /query or fragment/
    );
  }
});

test('System One endpoint is canonical', () => {
  assert.equal(
    openJevSystemOneUrl('https://jev.example/v1'),
    'https://jev.example/v1/systemone'
  );
  assert.equal(
    openJevSystemOneUrl('https://jev.example'),
    'https://jev.example/v1/systemone'
  );
  assert.equal(
    openJevSystemOneUrl('https://jev.example/v1/systemone'),
    'https://jev.example/v1/systemone'
  );
});

test('v2 request is sent as bounded state', async () => {
  let seen: {
    url?: string;
    payload?: Readonly<Record<string, unknown>>;
    headers?: Readonly<Record<string, string>>;
    timeoutMs?: number;
  } = {};
  const transport: OpenJevTransport = async (url, payload, headers, timeoutMs) => {
    seen = { url, payload, headers, timeoutMs };
    return goodResponse();
  };
  const model = new OpenJevDecisionModel(
    {
      baseUrl: 'https://jev.example',
      apiKey: 'secret',
      model: 'openjev-test',
      timeoutMs: 1250,
    },
    transport
  );
  const result = await model.score(request());
  assert.equal(result.selectedId, 'submit');
  assert.equal(result.model, 'openjev-fixture');
  assert.equal(seen.url, 'https://jev.example/v1/systemone');
  assert.equal(seen.headers?.Authorization, 'Bearer secret');
  assert.equal(seen.timeoutMs, 1250);
  const payload = seen.payload as {
    state: ReturnType<typeof request>;
    questions: { candidate: { criteria: Record<string, string> } };
  };
  assert.equal(payload.state.schema, REQUEST_SCHEMA_V2);
  assert.equal(payload.state.snapshot_id, 'snap-1');
  assert.equal(payload.state.elements[0].label, 'Submit');
  assert.deepEqual(Object.keys(payload.questions.candidate.criteria), [
    'submit',
    'reobserve',
    'abstain',
  ]);
});

test('unknown candidate is rejected', async () => {
  const bad: any = goodResponse();
  bad.answers.candidate.choice = 'not-supplied';
  bad.answers.candidate.probabilities = {
    submit: 0.05,
    reobserve: 0.05,
    abstain: 0.0,
    'not-supplied': 0.9,
  };
  const model = new OpenJevDecisionModel(
    { baseUrl: 'https://jev.example', apiKey: '', model: 'openjev', timeoutMs: 1000 },
    async () => bad,
  );
  await assert.rejects(model.score(request()), OpenJevError);
});

test('bad probability mass and argmax are rejected', async () => {
  const bad = goodResponse();
  bad.answers.candidate.choice = 'reobserve';
  bad.answers.candidate.confidence = 0.4;
  bad.answers.candidate.probabilities = {
    submit: 0.5,
    reobserve: 0.4,
    abstain: 0.1,
  };
  const model = new OpenJevDecisionModel(
    { baseUrl: 'https://jev.example', apiKey: '', model: 'openjev', timeoutMs: 1000 },
    async () => bad,
  );
  await assert.rejects(model.score(request()), /argmax/);
});

test('missing answers is bounded invalid_response', async () => {
  const model = new OpenJevDecisionModel(
    { baseUrl: 'https://jev.example', apiKey: '', model: 'openjev', timeoutMs: 1000 },
    async () => ({ model: 'x' }),
  );
  await assert.rejects(
    model.score(request()),
    (error: unknown) => error instanceof OpenJevError && error.code === 'invalid_response'
  );
});

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T | PromiseLike<T>) => void;
  reject: (reason?: unknown) => void;
};

function deferred<T>(): Deferred<T> {
  return (
    Promise as PromiseConstructor & { withResolvers<T>(): Deferred<T> }
  ).withResolvers<T>();
}

type Loopback = {
  baseUrl: string;
  hits: string[];
  close: () => Promise<void>;
};

function listen(
  respond: (path: string) => { status: number; body: Buffer; headers?: Record<string, string> }
): Promise<Loopback> {
  const hits: string[] = [];
  const server: Server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      const path = req.url ?? '/';
      hits.push(path);
      const reply = respond(path);
      res.writeHead(reply.status, {
        'Content-Type': 'application/json',
        'Content-Length': String(reply.body.length),
        ...reply.headers,
      });
      res.end(reply.body);
    });
  });
  const { promise, resolve } = deferred<Loopback>();
  server.listen(0, '127.0.0.1', () => {
    const address = server.address() as AddressInfo;
    resolve({
      baseUrl: `http://127.0.0.1:${address.port}`,
      hits,
      close: () => {
        const closed = deferred<void>();
        server.close((error) => (error ? closed.reject(error) : closed.resolve()));
        return closed.promise;
      },
    });
  });
  return promise;
}

function liveModel(baseUrl: string): OpenJevDecisionModel {
  return new OpenJevDecisionModel({
    baseUrl,
    apiKey: '',
    model: 'openjev',
    timeoutMs: 2000,
  });
}

test('loopback valid response selects the supplied id once', async () => {
  const server = await listen(() => ({
    status: 200,
    body: Buffer.from(JSON.stringify(goodResponse())),
  }));
  try {
    const result = await liveModel(server.baseUrl).score(request());
    assert.equal(result.selectedId, 'submit');
    assert.deepEqual(server.hits, ['/v1/systemone']);
  } finally {
    await server.close();
  }
});

test('loopback unknown id, 503, and malformed body do not retry', async () => {
  const unknown = goodResponse();
  unknown.answers.candidate.choice = 'not-supplied';
  unknown.answers.candidate.probabilities = {
    'not-supplied': 0.9,
    reobserve: 0.05,
    abstain: 0.05,
  };
  const cases = [
    { name: 'unknown', status: 200, body: JSON.stringify(unknown), code: 'invalid_response' },
    { name: 'unavailable', status: 503, body: 'unavailable', code: 'http_error' },
    { name: 'malformed', status: 200, body: 'not-json', code: 'invalid_response' },
  ];
  for (const item of cases) {
    const server = await listen(() => ({
      status: item.status,
      body: Buffer.from(item.body),
    }));
    try {
      await assert.rejects(
        liveModel(server.baseUrl).score(request()),
        (error: unknown) => error instanceof OpenJevError && error.code === item.code
      );
      assert.deepEqual(server.hits, ['/v1/systemone']);
    } finally {
      await server.close();
    }
  }
});

test('loopback redirect is refused and not followed', async () => {
  const server = await listen((path) => {
    if (path === '/stolen') {
      return { status: 200, body: Buffer.from('{"followed":true}') };
    }
    return {
      status: 302,
      body: Buffer.alloc(0),
      headers: { Location: '/stolen' },
    };
  });
  try {
    await assert.rejects(
      liveModel(server.baseUrl).score(request()),
      (error: unknown) => error instanceof OpenJevError && error.code === 'http_error'
    );
    assert.deepEqual(server.hits, ['/v1/systemone']);
  } finally {
    await server.close();
  }
});

test('loopback oversized response is refused', async () => {
  const server = await listen(() => ({
    status: 200,
    body: Buffer.alloc(256 * 1024 + 1, 0x78),
  }));
  try {
    await assert.rejects(
      liveModel(server.baseUrl).score(request()),
      (error: unknown) => error instanceof OpenJevError && error.code === 'response_too_large'
    );
    assert.deepEqual(server.hits, ['/v1/systemone']);
  } finally {
    await server.close();
  }
});
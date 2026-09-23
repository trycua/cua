import assert from 'node:assert/strict';
import test from 'node:test';

import {
  DEFAULT_LOCAL_JEV_URL,
  DEFAULT_TYPESAFE_BASE_URL,
  JevProtocolError,
  JevTransportError,
  SystemOneHttpClient,
  chooseWithBackend,
  describeBackend,
  readJevConfig,
  systemoneUrl,
  validateChoiceAnswer,
  validateCriteria,
  validateLoopbackUrl,
  validateRemoteUrl,
  type SystemOneTransport,
} from './jev_backends.js';

const CRITERIA = {
  'type-verification-value': 'Replace the verification field.',
  reobserve: 'Obtain a fresh observation.',
  abstain: 'Stop without acting.',
};

function goodAnswers(choice = 'type-verification-value', confidence = 0.9) {
  const rest = (1 - confidence) / 2;
  return {
    candidate: {
      type: 'choice',
      choice,
      confidence,
      probabilities: Object.fromEntries(
        Object.keys(CRITERIA).map((id) => [id, id === choice ? confidence : rest])
      ),
    },
  };
}

function stubTransport(response: Record<string, unknown>): SystemOneTransport {
  return async () => response;
}

test('config defaults to mock', () => {
  const config = readJevConfig({});
  assert.equal(config.backend, 'mock');
  assert.equal(config.model, 'jev-latest');
  assert.equal(config.timeoutMs, 2500);
});

test('config honors live alias and rejects unknown backends', () => {
  assert.equal(readJevConfig({ JEV_BACKEND: 'live' }).backend, 'typesafe');
  assert.throws(() => readJevConfig({ JEV_BACKEND: 'nope' }), /JEV_BACKEND/);
});

test('config honors typesafe aliases', () => {
  const config = readJevConfig({
    JEV_BACKEND: 'typesafe',
    TYPESAFE_API_KEY: 'k',
    TYPESAFE_MODEL: 'm',
  });
  assert.equal(config.apiKey, 'k');
  assert.equal(config.model, 'm');
  assert.equal(config.baseUrl, DEFAULT_TYPESAFE_BASE_URL);
});

test('local backend defaults to loopback without a key', () => {
  const config = readJevConfig({ JEV_BACKEND: 'local' });
  assert.equal(config.baseUrl, DEFAULT_LOCAL_JEV_URL);
  assert.equal(config.apiKey, '');
});

test('bad timeout falls back to the default', () => {
  assert.equal(readJevConfig({ JEV_TIMEOUT_MS: 'soon' }).timeoutMs, 2500);
});

test('systemoneUrl appends the endpoint', () => {
  assert.equal(systemoneUrl('http://127.0.0.1:8787'), 'http://127.0.0.1:8787/v1/systemone');
  assert.equal(
    systemoneUrl('https://api.typesafe.ai/v1/'),
    'https://api.typesafe.ai/v1/systemone'
  );
});

test('validateLoopbackUrl keeps local Jev on loopback', () => {
  assert.equal(validateLoopbackUrl('http://127.0.0.1:8787/'), 'http://127.0.0.1:8787');
  for (const bad of [
    'https://127.0.0.1:8787',
    'http://example.com:8787',
    'http://user:pass@127.0.0.1:8787',
  ]) {
    assert.throws(() => validateLoopbackUrl(bad), /local Jev backend/);
  }
});

test('validateRemoteUrl never sends credentials over plaintext', () => {
  assert.equal(validateRemoteUrl('https://jev.example/v1', true), 'https://jev.example/v1');
  assert.equal(validateRemoteUrl('http://jev.example/v1', false), 'http://jev.example/v1');
  assert.throws(() => validateRemoteUrl('http://jev.example/v1', true), /https/);
  assert.throws(
    () => validateRemoteUrl('https://user:pass@jev.example/v1', false),
    /embed credentials/
  );
});

test('validateChoiceAnswer accepts a well-formed answer', () => {
  const answer = validateChoiceAnswer('candidate', goodAnswers().candidate, new Set(Object.keys(CRITERIA)));
  assert.equal(answer.choice, 'type-verification-value');
});

test('validateChoiceAnswer rejects an unknown choice', () => {
  const answers = goodAnswers();
  answers.candidate.choice = 'nope';
  answers.candidate.probabilities = {
    nope: 0.9,
    reobserve: 0.05,
    abstain: 0.05,
  };
  assert.throws(
    () => validateChoiceAnswer('candidate', answers.candidate, new Set(Object.keys(CRITERIA))),
    JevProtocolError
  );
});

test('validateChoiceAnswer rejects bad probability mass', () => {
  const answers = goodAnswers();
  answers.candidate.probabilities.abstain = 0.9; // mass 1.81
  assert.throws(
    () => validateChoiceAnswer('candidate', answers.candidate, new Set(Object.keys(CRITERIA))),
    /mass/
  );
});

test('validateChoiceAnswer rejects an argmax mismatch', () => {
  const answers = goodAnswers('reobserve', 0.34);
  answers.candidate.probabilities = {
    'type-verification-value': 0.5,
    reobserve: 0.34,
    abstain: 0.16,
  }; // mass 1.0, but the argmax is not the choice
  assert.throws(
    () => validateChoiceAnswer('candidate', answers.candidate, new Set(Object.keys(CRITERIA))),
    /argmax/
  );
});

test('validateChoiceAnswer rejects non-finite values', () => {
  const answers = goodAnswers();
  answers.candidate.confidence = Number.POSITIVE_INFINITY;
  assert.throws(
    () => validateChoiceAnswer('candidate', answers.candidate, new Set(Object.keys(CRITERIA))),
    JevProtocolError
  );
});

test('validateCriteria rejects empty mappings and empty descriptions', () => {
  assert.throws(() => validateCriteria({}), /non-empty/);
  assert.throws(() => validateCriteria({ a: '' }), /description/);
});

test('mock backend is deterministic', async () => {
  const outcome = await chooseWithBackend(readJevConfig({}), {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
  });
  assert.equal(outcome.ok, true);
  assert.equal(outcome.decision?.selectedId, 'type-verification-value');
  assert.equal(
    Object.values(outcome.decision?.probabilities ?? {}).reduce((a, b) => a + b, 0),
    1
  );
});

test('typesafe without a key skips fail-open', async () => {
  const outcome = await chooseWithBackend(readJevConfig({ JEV_BACKEND: 'typesafe' }), {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
  });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'missing_credentials');
});

test('typesafe key is not sent to a plaintext override', async () => {
  const transport: SystemOneTransport = async () => {
    throw new Error('transport must not run');
  };
  const config = readJevConfig({
    JEV_BACKEND: 'typesafe',
    JEV_API_KEY: 'secret',
    JEV_BASE_URL: 'http://jev.example',
  });
  const outcome = await chooseWithBackend(config, {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
    transport,
  });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'validation_error');
  assert.match(outcome.message ?? '', /https/);
});

test('openjev without a URL skips fail-open', async () => {
  const outcome = await chooseWithBackend(readJevConfig({ JEV_BACKEND: 'openjev' }), {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
  });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'missing_base_url');
});

test('local backend rejects non-loopback URLs', async () => {
  const config = readJevConfig({ JEV_BACKEND: 'local', JEV_BASE_URL: 'http://example.com:8787' });
  const outcome = await chooseWithBackend(config, {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
  });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'validation_error');
});

test('local backend happy path', async () => {
  const outcome = await chooseWithBackend(readJevConfig({ JEV_BACKEND: 'local' }), {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
    transport: stubTransport({ model: 'local-jev', answers: goodAnswers() }),
  });
  assert.equal(outcome.ok, true);
  assert.equal(outcome.decision?.selectedId, 'type-verification-value');
  assert.equal(outcome.decision?.model, 'local-jev');
  assert.equal(outcome.decision?.backend, 'local');
});

test('timeout maps to the timeout skip reason', async () => {
  const transport: SystemOneTransport = async () => {
    throw new JevTransportError('request timed out: timed out');
  };
  const outcome = await chooseWithBackend(readJevConfig({ JEV_BACKEND: 'local' }), {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
    transport,
  });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'timeout');
});

test('HTTP failure maps to the http_error skip reason', async () => {
  const transport: SystemOneTransport = async () => {
    throw new JevTransportError('HTTP 500: boom');
  };
  const outcome = await chooseWithBackend(readJevConfig({ JEV_BACKEND: 'local' }), {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
    transport,
  });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'http_error');
});

test('malformed model output is invalid_response, never trusted', async () => {
  const answers = goodAnswers();
  answers.candidate.probabilities.abstain = 0.9; // mass 1.81
  const outcome = await chooseWithBackend(readJevConfig({ JEV_BACKEND: 'local' }), {
    goal: 'g',
    observation: {},
    criteria: CRITERIA,
    transport: stubTransport({ answers }),
  });
  assert.equal(outcome.ok, false);
  assert.equal(outcome.reason, 'invalid_response');
});

test('describeBackend redacts the key', () => {
  const described = describeBackend(
    readJevConfig({ JEV_BACKEND: 'typesafe', JEV_API_KEY: 'secret-key' })
  );
  assert.equal(described.has_api_key, true);
  assert.ok(!JSON.stringify(described).includes('secret-key'));
});

test('ask throws on a missing answers object', async () => {
  const client = new SystemOneHttpClient(
    readJevConfig({ JEV_BACKEND: 'local' }),
    stubTransport({ model: 'x' })
  );
  await assert.rejects(client.ask({ state: {}, questions: {} }), JevProtocolError);
});

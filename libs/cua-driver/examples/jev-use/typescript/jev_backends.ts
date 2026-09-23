/**
 * Generalized Jev backends for the jev-use example.
 *
 * Ports the fail-open Jev adapter semantics from Kevin's Hermes / oh-my-pi work
 * (agentweb `jevAdapter.ts`, Hermes `tools/computer_use/system_one.py` and the
 * `computer.decide()` decision lane) into the cua jev-use example, and adds a
 * first-class backend for a locally running Jev.
 *
 * Backends:
 * - `mock`: deterministic credential-free chooser. Default; used by CI and dry runs.
 * - `typesafe`: TypeSafe cloud System One. Requires `JEV_API_KEY` (or `TYPESAFE_API_KEY`).
 * - `openjev`: any OpenJev-compatible System One HTTP endpoint. Requires `JEV_BASE_URL`
 *   (or `OPENJEV_BASE_URL`); the API key is optional.
 * - `local`: a locally running Jev on loopback (default `http://127.0.0.1:8787`).
 *   The URL must stay on loopback; no API key is required.
 *
 * All HTTP backends speak the same wire format -- `POST {base}/v1/systemone` with
 * `{state, model, questions}` returning `{model, answers, usage}` -- with no SDK
 * dependency. Transport problems never throw into the caller: they come back as a
 * skipped {@link JevOutcome} with a machine-readable reason, mirroring the
 * Hermes/agentweb fail-open contract. Malformed model output is fail-closed
 * instead: it is reported as `invalid_response` rather than trusted.
 */

export type JevBackendName = 'mock' | 'typesafe' | 'openjev' | 'local';

export const BACKENDS: readonly JevBackendName[] = ['mock', 'typesafe', 'openjev', 'local'];

/** Skip reasons for the fail-open envelope. `invalid_response` is a live
 * variant: malformed model output is reported, never trusted. */
export type JevSkipReason =
  | 'disabled'
  | 'missing_credentials'
  | 'missing_base_url'
  | 'timeout'
  | 'http_error'
  | 'invalid_response'
  | 'validation_error';

export const DEFAULT_MODEL = 'jev-latest';
export const DEFAULT_TIMEOUT_MS = 2500;
export const DEFAULT_TYPESAFE_BASE_URL = 'https://api.typesafe.ai';
export const DEFAULT_LOCAL_JEV_URL = 'http://127.0.0.1:8787';

const RESERVED_IDS: ReadonlySet<string> = new Set(['reobserve', 'abstain']);
const LOOPBACK_HOSTS: ReadonlySet<string> = new Set([
  '127.0.0.1',
  'localhost',
  '::1',
  '[::1]',
]);

export class JevTransportError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'JevTransportError';
  }
}

export class JevProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'JevProtocolError';
  }
}

export type JevConfig = Readonly<{
  backend: JevBackendName;
  baseUrl: string;
  apiKey: string;
  model: string;
  timeoutMs: number;
}>;

function envValue(env: Record<string, string | undefined>, ...names: string[]): string {
  for (const name of names) {
    const value = env[name];
    if (value !== undefined && value.trim()) return value.trim();
  }
  return '';
}

/**
 * Read the Jev backend configuration from the environment.
 *
 * `JEV_BACKEND` selects `mock` (default), `typesafe`, `openjev` or `local`;
 * `live` is accepted as an alias for `typesafe`. An explicit unknown backend
 * is rejected rather than silently becoming `mock`. `TYPESAFE_*` variables
 * are honored as aliases for the shared
 * settings.
 */
export function readJevConfig(
  env: Record<string, string | undefined> = process.env
): JevConfig {
  let backend = envValue(env, 'JEV_BACKEND').toLowerCase() || 'mock';
  if (backend === 'live') backend = 'typesafe';
  if (!(BACKENDS as readonly string[]).includes(backend)) {
    throw new Error(
      `JEV_BACKEND must be one of ${BACKENDS.join(', ')} (or deprecated live), got ${JSON.stringify(backend)}`
    );
  }
  const resolved = backend as JevBackendName;

  let baseUrl = envValue(env, 'JEV_BASE_URL', 'TYPESAFE_BASE_URL', 'OPENJEV_BASE_URL');
  if (!baseUrl) {
    if (resolved === 'typesafe') baseUrl = DEFAULT_TYPESAFE_BASE_URL;
    else if (resolved === 'local') baseUrl = DEFAULT_LOCAL_JEV_URL;
  }

  const timeoutRaw = envValue(env, 'JEV_TIMEOUT_MS', 'TYPESAFE_TIMEOUT_MS');
  let timeoutMs = Number.parseInt(timeoutRaw, 10);
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) timeoutMs = DEFAULT_TIMEOUT_MS;

  return Object.freeze({
    backend: resolved,
    baseUrl: baseUrl.replace(/\/+$/, ''),
    apiKey: envValue(env, 'JEV_API_KEY', 'TYPESAFE_API_KEY'),
    model: envValue(env, 'JEV_MODEL', 'TYPESAFE_MODEL') || DEFAULT_MODEL,
    timeoutMs,
  });
}

/** Return the System One endpoint for a base URL. */
export function systemoneUrl(baseUrl: string): string {
  const root = baseUrl.replace(/\/+$/, '');
  return root.endsWith('/v1') ? `${root}/systemone` : `${root}/v1/systemone`;
}

/** Ensure a `local` backend URL stays on loopback. Returns the normalized URL. */
export function validateLoopbackUrl(url: string): string {
  const parsed = new URL(url);
  if (parsed.protocol !== 'http:' || !LOOPBACK_HOSTS.has(parsed.hostname)) {
    throw new Error('local Jev backend must be an http:// loopback URL');
  }
  if (parsed.username || parsed.password) {
    throw new Error('local Jev backend URL must not embed credentials');
  }
  return url.replace(/\/+$/, '');
}

/** Validate a non-local backend URL before task state or credentials leave the host. */
export function validateRemoteUrl(url: string, hasApiKey: boolean): string {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new Error('remote Jev backend must use an http:// or https:// URL');
  }
  if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname) {
    throw new Error('remote Jev backend must use an http:// or https:// URL');
  }
  if (parsed.username || parsed.password) {
    throw new Error('remote Jev backend URL must not embed credentials');
  }
  if (hasApiKey && parsed.protocol !== 'https:') {
    throw new Error('Jev API keys may only be sent to an https:// backend');
  }
  return url.replace(/\/+$/, '');
}

export type ChoiceAnswer = Readonly<{
  questionId: string;
  choice: string;
  confidence: number;
  probabilities: Readonly<Record<string, number>>;
}>;

function isFiniteUnit(value: unknown): value is number {
  return (
    typeof value === 'number' &&
    Number.isFinite(value) &&
    value >= 0 &&
    value <= 1
  );
}

/**
 * Fail-closed validation of one System One choice answer.
 *
 * Ports oh-my-pi's `validateChoiceAnswer`: the probability mass must be ~1
 * (tolerance 0.02), every value finite in [0,1], the key set must equal the
 * allowed candidate ids exactly, and the choice must be the argmax. Anything
 * else throws {@link JevProtocolError}.
 */
export function validateChoiceAnswer(
  questionId: string,
  answer: unknown,
  allowedIds: ReadonlySet<string>
): ChoiceAnswer {
  if (!answer || typeof answer !== 'object' || (answer as Record<string, unknown>).type !== 'choice') {
    throw new JevProtocolError(`question '${questionId}': expected a choice answer`);
  }
  const record = answer as Record<string, unknown>;
  const choice = record.choice;
  if (typeof choice !== 'string' || !allowedIds.has(choice)) {
    throw new JevProtocolError(`question '${questionId}': unknown choice ${JSON.stringify(choice)}`);
  }
  if (!isFiniteUnit(record.confidence)) {
    throw new JevProtocolError(`question '${questionId}': invalid confidence`);
  }
  const rawProbs = record.probabilities;
  if (!rawProbs || typeof rawProbs !== 'object' || Array.isArray(rawProbs)) {
    throw new JevProtocolError(`question '${questionId}': missing probabilities`);
  }
  const probabilities: Record<string, number> = {};
  for (const [candidateId, value] of Object.entries(rawProbs as Record<string, unknown>)) {
    if (!allowedIds.has(candidateId)) {
      throw new JevProtocolError(
        `question '${questionId}': probability for unknown id '${candidateId}'`
      );
    }
    if (!isFiniteUnit(value)) {
      throw new JevProtocolError(
        `question '${questionId}': invalid probability for '${candidateId}'`
      );
    }
    probabilities[candidateId] = value;
  }
  if (
    Object.keys(probabilities).length !== allowedIds.size ||
    !Object.keys(probabilities).every((id) => allowedIds.has(id))
  ) {
    throw new JevProtocolError(
      `question '${questionId}': probability keys do not match candidate ids`
    );
  }
  const mass = Object.values(probabilities).reduce((sum, value) => sum + value, 0);
  if (Math.abs(mass - 1) > 0.02) {
    throw new JevProtocolError(
      `question '${questionId}': probability mass ${mass.toFixed(3)} != 1`
    );
  }
  const argmax = Object.keys(probabilities).reduce((best, id) =>
    probabilities[id] > probabilities[best] ? id : best
  );
  if (argmax !== choice) {
    throw new JevProtocolError(
      `question '${questionId}': choice '${choice}' is not the argmax '${argmax}'`
    );
  }
  return Object.freeze({
    questionId,
    choice,
    confidence: record.confidence as number,
    probabilities: Object.freeze(probabilities),
  });
}

/** Validate one System One noul answer, returning the probability. */
export function validateNoulAnswer(questionId: string, answer: unknown): number {
  if (!answer || typeof answer !== 'object' || (answer as Record<string, unknown>).type !== 'noul') {
    throw new JevProtocolError(`question '${questionId}': expected a noul answer`);
  }
  const value = (answer as Record<string, unknown>).noul;
  if (!isFiniteUnit(value)) {
    throw new JevProtocolError(`question '${questionId}': invalid noul value`);
  }
  return value;
}

export type SystemOneTransport = (
  url: string,
  payload: Record<string, unknown>,
  headers: Record<string, string>,
  timeoutSeconds: number
) => Promise<Record<string, unknown>>;

export function systemOneRequestInit(
  payload: Record<string, unknown>,
  headers: Record<string, string>,
  timeoutSeconds: number
): RequestInit {
  return {
    method: 'POST',
    headers,
    body: JSON.stringify(payload),
    redirect: 'error',
    signal: AbortSignal.timeout(timeoutSeconds * 1000),
  };
}

async function defaultTransport(
  url: string,
  payload: Record<string, unknown>,
  headers: Record<string, string>,
  timeoutSeconds: number
): Promise<Record<string, unknown>> {
  let response: Response;
  try {
    response = await fetch(url, systemOneRequestInit(payload, headers, timeoutSeconds));
  } catch (error: unknown) {
    if (error instanceof Error && error.name === 'TimeoutError') {
      throw new JevTransportError(`request timed out: ${error.message}`);
    }
    throw new JevTransportError(
      `transport error: ${error instanceof Error ? error.message : String(error)}`
    );
  }
  if (!response.ok) {
    const detail = await response.text().catch(() => '');
    throw new JevTransportError(
      `HTTP ${response.status}${detail ? `: ${detail.slice(0, 200)}` : ''}`
    );
  }
  let parsed: unknown;
  try {
    parsed = await response.json();
  } catch (error: unknown) {
    throw new JevProtocolError(
      `response is not JSON: ${error instanceof Error ? error.message : String(error)}`
    );
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new JevProtocolError('response was not a JSON object');
  }
  return parsed as Record<string, unknown>;
}

export type SystemOneResponse = Readonly<{
  model: string;
  answers: Record<string, unknown>;
  usage?: Record<string, unknown>;
}>;

/**
 * Minimal System One HTTP client with no SDK dependency.
 *
 * `transport` is injectable for tests; the default performs a real `fetch`
 * POST with a timeout.
 */
export class SystemOneHttpClient {
  constructor(
    private readonly config: JevConfig,
    private readonly transport: SystemOneTransport = defaultTransport
  ) {}

  async ask(args: {
    state: Record<string, unknown>;
    questions: Record<string, unknown>;
  }): Promise<SystemOneResponse> {
    if (this.config.backend === 'local') {
      validateLoopbackUrl(this.config.baseUrl || DEFAULT_LOCAL_JEV_URL);
    } else if (this.config.backend === 'typesafe' || this.config.backend === 'openjev') {
      validateRemoteUrl(this.config.baseUrl, Boolean(this.config.apiKey));
    }
    const baseUrl = this.config.baseUrl;
    if (!baseUrl) throw new JevTransportError('no base URL configured');
    const headers: Record<string, string> = {
      Accept: 'application/json',
      'Content-Type': 'application/json',
    };
    if (this.config.apiKey) headers.Authorization = `Bearer ${this.config.apiKey}`;
    const parsed = await this.transport(
      systemoneUrl(baseUrl),
      { state: { ...args.state }, model: this.config.model, questions: { ...args.questions } },
      headers,
      this.config.timeoutMs / 1000
    );
    const answers = parsed.answers;
    if (!answers || typeof answers !== 'object' || Array.isArray(answers)) {
      throw new JevProtocolError('response has no answers object');
    }
    const model =
      typeof parsed.model === 'string' && parsed.model.trim()
        ? parsed.model
        : this.config.model;
    const usage =
      parsed.usage && typeof parsed.usage === 'object' && !Array.isArray(parsed.usage)
        ? (parsed.usage as Record<string, unknown>)
        : undefined;
    return Object.freeze({
      model,
      answers: answers as Record<string, unknown>,
      ...(usage ? { usage } : {}),
    });
  }
}

export type JevDecision = Readonly<{
  selectedId: string;
  confidence: number;
  probabilities: Readonly<Record<string, number>>;
  model?: string;
  backend: JevBackendName;
}>;

/** Fail-open envelope: `ok` carries a decision, otherwise `reason`. */
export type JevOutcome = Readonly<{
  ok: boolean;
  decision?: JevDecision;
  reason?: JevSkipReason;
  message?: string;
  backend: JevBackendName;
}>;

function skipped(
  backend: JevBackendName,
  reason: JevSkipReason,
  message: string
): JevOutcome {
  return Object.freeze({ ok: false, reason, message, backend });
}

export function validateCriteria(criteria: Record<string, string>): Record<string, string> {
  const cleaned: Record<string, string> = {};
  const entries = Object.entries(criteria ?? {});
  if (!entries.length) throw new Error('criteria must be a non-empty mapping');
  for (const [candidateId, description] of entries) {
    if (typeof candidateId !== 'string' || !candidateId.trim()) {
      throw new Error('candidate ids must be non-empty strings');
    }
    if (typeof description !== 'string' || !description.trim()) {
      throw new Error(`candidate '${candidateId}' needs a description`);
    }
    if (Object.hasOwn(cleaned, candidateId)) {
      throw new Error('candidate set contains duplicate IDs');
    }
    cleaned[candidateId] = description;
  }
  return cleaned;
}

/**
 * Choose one candidate id through the configured Jev backend.
 *
 * Never throws for backend problems: transport failures, timeouts and
 * malformed model output come back as a skipped {@link JevOutcome} with a
 * reason. Only caller-side misuse (bad criteria) throws.
 */
export async function chooseWithBackend(
  config: JevConfig,
  args: {
    goal: string;
    observation: Record<string, unknown>;
    criteria: Record<string, string>;
    transport?: SystemOneTransport;
  }
): Promise<JevOutcome> {
  let cleaned: Record<string, string>;
  try {
    cleaned = validateCriteria(args.criteria);
  } catch (error: unknown) {
    return skipped(config.backend, 'validation_error', error instanceof Error ? error.message : String(error));
  }
  if (typeof args.goal !== 'string' || !args.goal.trim()) {
    return skipped(config.backend, 'validation_error', 'goal must be non-empty');
  }

  if (config.backend === 'mock') {
    const ids = Object.keys(cleaned);
    const selected =
      ids.find((id) => !RESERVED_IDS.has(id)) ??
      (ids.includes('reobserve') ? 'reobserve' : ids[0]);
    return Object.freeze({
      ok: true,
      decision: Object.freeze({
        selectedId: selected,
        confidence: 1,
        probabilities: Object.freeze(
          Object.fromEntries(ids.map((id) => [id, id === selected ? 1 : 0]))
        ),
        model: 'mock',
        backend: 'mock' as const,
      }),
      backend: 'mock' as const,
    });
  }

  if (config.backend === 'typesafe' && !config.apiKey) {
    return skipped(
      config.backend,
      'missing_credentials',
      'JEV_BACKEND=typesafe but no API key is set ' +
        '(JEV_API_KEY or TYPESAFE_API_KEY); skipped fail-open.'
    );
  }
  if (config.backend === 'openjev' && !config.baseUrl) {
    return skipped(
      config.backend,
      'missing_base_url',
      'JEV_BACKEND=openjev but no base URL is set ' +
        '(JEV_BASE_URL or OPENJEV_BASE_URL); skipped fail-open.'
    );
  }
  if (config.backend === 'local') {
    try {
      validateLoopbackUrl(config.baseUrl || DEFAULT_LOCAL_JEV_URL);
    } catch (error: unknown) {
      return skipped(
        config.backend,
        'validation_error',
        error instanceof Error ? error.message : String(error)
      );
    }
  } else if (config.backend === 'typesafe' || config.backend === 'openjev') {
    try {
      validateRemoteUrl(config.baseUrl, Boolean(config.apiKey));
    } catch (error: unknown) {
      return skipped(
        config.backend,
        'validation_error',
        error instanceof Error ? error.message : String(error)
      );
    }
  }

  const client = new SystemOneHttpClient(config, args.transport);
  try {
    const response = await client.ask({
      state: { goal: args.goal, observation: { ...args.observation } },
      questions: {
        candidate: {
          type: 'choice',
          instructions: 'Select exactly one supplied candidate ID.',
          criteria: cleaned,
        },
      },
    });
    const answer = validateChoiceAnswer(
      'candidate',
      response.answers.candidate,
      new Set(Object.keys(cleaned))
    );
    return Object.freeze({
      ok: true,
      decision: Object.freeze({
        selectedId: answer.choice,
        confidence: answer.confidence,
        probabilities: answer.probabilities,
        model: response.model,
        backend: config.backend,
      }),
      backend: config.backend,
    });
  } catch (error: unknown) {
    if (error instanceof JevTransportError) {
      const message = error.message;
      return skipped(
        config.backend,
        message.includes('timed out') ? 'timeout' : 'http_error',
        message
      );
    }
    if (error instanceof JevProtocolError) {
      return skipped(config.backend, 'invalid_response', error.message);
    }
    throw error;
  }
}

/** Redacted backend description for logs and evidence (no secrets). */
export function describeBackend(config: JevConfig): Record<string, unknown> {
  return {
    backend: config.backend,
    base_url: config.baseUrl,
    model: config.model,
    timeout_ms: config.timeoutMs,
    has_api_key: Boolean(config.apiKey),
  };
}

/**
 * OpenJev System One adapter for the current jev-use request contract.
 *
 * This module scores only caller-supplied candidate IDs. It does not construct
 * Driver actions, retry mutations, or own routing policy.
 */
import { validateRequest } from './choose_action.js';

export const DEFAULT_OPENJEV_MODEL = 'openjev';
export const DEFAULT_OPENJEV_TIMEOUT_MS = 2500;
export const MAX_OPENJEV_TIMEOUT_MS = 60000;
export const MAX_OPENJEV_RESPONSE_BYTES = 256 * 1024;

export type OpenJevErrorCode =
  | 'missing_base_url'
  | 'invalid_url'
  | 'insecure_credentials'
  | 'timeout'
  | 'http_error'
  | 'response_too_large'
  | 'invalid_response';

export class OpenJevError extends Error {
  constructor(public readonly code: OpenJevErrorCode, message: string) {
    super(message);
    this.name = 'OpenJevError';
  }
}

export type OpenJevConfig = Readonly<{
  baseUrl: string;
  apiKey: string;
  model: string;
  timeoutMs: number;
}>;

export type OpenJevScores = Readonly<{
  selectedId: string;
  confidence: number;
  probabilities: Readonly<Record<string, number>>;
  model: string;
}>;

export type OpenJevTransport = (
  url: string,
  payload: Readonly<Record<string, unknown>>,
  headers: Readonly<Record<string, string>>,
  timeoutMs: number,
) => Promise<unknown>;

export function readOpenJevConfig(
  env: Readonly<Record<string, string | undefined>> = process.env
): OpenJevConfig {
  const rawTimeout = Number.parseInt((env.OPENJEV_TIMEOUT_MS ?? '').trim(), 10);
  const timeoutMs = Number.isFinite(rawTimeout)
    ? Math.min(MAX_OPENJEV_TIMEOUT_MS, Math.max(100, rawTimeout))
    : DEFAULT_OPENJEV_TIMEOUT_MS;
  return Object.freeze({
    baseUrl: (env.OPENJEV_BASE_URL ?? '').trim(),
    apiKey: (env.OPENJEV_API_KEY ?? '').trim(),
    model: (env.OPENJEV_MODEL ?? '').trim() || DEFAULT_OPENJEV_MODEL,
    timeoutMs,
  });
}

export function validateOpenJevBaseUrl(baseUrl: string, hasApiKey: boolean): string {
  const value = baseUrl.trim().replace(/\/+$/, '');
  if (!value) throw new OpenJevError('missing_base_url', 'OPENJEV_BASE_URL is not set');
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    throw new OpenJevError('invalid_url', 'OPENJEV_BASE_URL is not a valid URL');
  }
  if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname) {
    throw new OpenJevError(
      'invalid_url',
      'OPENJEV_BASE_URL must use http:// or https:// and include a host'
    );
  }
  if (parsed.username || parsed.password) {
    throw new OpenJevError('invalid_url', 'OPENJEV_BASE_URL must not embed credentials');
  }
  if (parsed.search || parsed.hash) {
    throw new OpenJevError(
      'invalid_url',
      'OPENJEV_BASE_URL must not contain a query or fragment'
    );
  }
  if (hasApiKey && parsed.protocol !== 'https:') {
    throw new OpenJevError(
      'insecure_credentials',
      'OPENJEV_API_KEY may only be sent to an https:// endpoint'
    );
  }
  return value;
}

export function openJevSystemOneUrl(baseUrl: string): string {
  const root = baseUrl.replace(/\/+$/, '');
  if (root.endsWith('/v1/systemone')) return root;
  return root.endsWith('/v1') ? root + '/systemone' : root + '/v1/systemone';
}

async function defaultTransport(
  url: string,
  payload: Readonly<Record<string, unknown>>,
  headers: Readonly<Record<string, string>>,
  timeoutMs: number
): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(url, {
      method: 'POST',
      headers: { ...headers },
      body: JSON.stringify(payload),
      redirect: 'error',
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (error) {
    const name = error instanceof Error ? error.name : '';
    if (name === 'TimeoutError' || name === 'AbortError') {
      throw new OpenJevError('timeout', 'OpenJev request timed out');
    }
    throw new OpenJevError(
      'http_error',
      name === 'TypeError'
        ? 'OpenJev redirect or transport was refused'
        : 'OpenJev endpoint is unreachable'
    );
  }
  if (!response.ok) {
    throw new OpenJevError(
      'http_error',
      'OpenJev endpoint returned HTTP ' + String(response.status)
    );
  }
  if (!response.body) {
    throw new OpenJevError('invalid_response', 'OpenJev response has no body');
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  const chunks: string[] = [];
  let bytes = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
    if (bytes > MAX_OPENJEV_RESPONSE_BYTES) {
      await reader.cancel();
      throw new OpenJevError('response_too_large', 'OpenJev response is too large');
    }
    chunks.push(decoder.decode(value, { stream: true }));
  }
  chunks.push(decoder.decode());
  try {
    return JSON.parse(chunks.join(''));
  } catch {
    throw new OpenJevError('invalid_response', 'OpenJev response is not JSON');
  }
}

function record(value: unknown, message: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new OpenJevError('invalid_response', message);
  }
  return value as Record<string, unknown>;
}

function finiteUnit(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 1;
}

export class OpenJevDecisionModel {
  readonly name = 'openjev';

  constructor(
    readonly config: OpenJevConfig = readOpenJevConfig(),
    private readonly transport: OpenJevTransport = defaultTransport,
  ) {}

  async score(request: unknown): Promise<OpenJevScores> {
    const validated = validateRequest(request);
    const baseUrl = validateOpenJevBaseUrl(this.config.baseUrl, Boolean(this.config.apiKey));
    const criteria = Object.fromEntries(
      validated.candidates.map(({ id, description }) => [id, description])
    );
    const payload = {
      model: this.config.model,
      state: validated,
      questions: {
        candidate: {
          type: 'choice',
          instructions: validated.goal,
          criteria,
        },
      },
    };
    const headers: Record<string, string> = {
      Accept: 'application/json',
      'Content-Type': 'application/json',
    };
    if (this.config.apiKey) headers.Authorization = 'Bearer ' + this.config.apiKey;
    const response = record(
      await this.transport(
        openJevSystemOneUrl(baseUrl),
        payload,
        headers,
        this.config.timeoutMs,
      ),
      'OpenJev response is not an object',
    );
    const answers = record(response.answers, 'OpenJev response has no answers object');
    const answer = record(
      answers.candidate,
      'OpenJev candidate answer is not an object',
    );
    if (answer.type !== 'choice') {
      throw new OpenJevError('invalid_response', 'OpenJev candidate answer is not a choice');
    }
    if (typeof answer.choice !== 'string') {
      throw new OpenJevError('invalid_response', 'OpenJev choice is not a string');
    }
    if (!finiteUnit(answer.confidence)) {
      throw new OpenJevError('invalid_response', 'OpenJev confidence is invalid');
    }
    const probabilities = record(
      answer.probabilities,
      'OpenJev probabilities are missing',
    );
    const ids = validated.candidates.map(({ id }) => id);
    if (
      Object.keys(probabilities).length !== ids.length ||
      !ids.every((id) => finiteUnit(probabilities[id]))
    ) {
      throw new OpenJevError(
        'invalid_response',
        'OpenJev probabilities do not match the candidate set',
      );
    }
    const numeric = Object.fromEntries(
      ids.map((id) => [id, probabilities[id] as number])
    );
    const mass = Object.values(numeric).reduce((sum, value) => sum + value, 0);
    if (Math.abs(mass - 1) > 0.02) {
      throw new OpenJevError('invalid_response', 'OpenJev probability mass is invalid');
    }
    const top = Math.max(...Object.values(numeric));
    const winners = ids.filter((id) => numeric[id] === top);
    if (winners.length !== 1 || winners[0] !== answer.choice) {
      throw new OpenJevError(
        'invalid_response',
        'OpenJev choice is not the unique probability argmax',
      );
    }
    const model =
      typeof response.model === 'string' && response.model.trim()
        ? response.model
        : this.config.model;
    return Object.freeze({
      selectedId: answer.choice,
      confidence: answer.confidence,
      probabilities: Object.freeze(numeric),
      model,
    });
  }
}
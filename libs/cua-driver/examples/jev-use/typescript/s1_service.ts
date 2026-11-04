/**
 * Choose one supplied candidate through a warm, loopback S1 decision service.
 *
 * Mirrors python/s1_service.py. `CUA_S1_DECISION_URL` names the decide
 * endpoint (for example `http://127.0.0.1:8791/decide`). The service receives
 * the validated `cua.jev_choice_request_v1` or `_v2` request and returns one
 * `cua.decision_choice_v1` response, which is checked strictly here before the
 * runner resolves the selected ID to its own immutable candidate.
 */
import { isIP } from 'node:net';

import { validateRequest } from './choose_action.js';

export const DECISION_SCHEMA = 'cua.decision_choice_v1';
export const URL_ENV = 'CUA_S1_DECISION_URL';
const DECISION_KINDS = new Set(['selected', 'reobserve', 'abstain', 'error']);
const DEFAULT_TIMEOUT_MS = 30_000;
const MAX_RESPONSE_BYTES = 256 * 1024;

export class S1ServiceError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'S1ServiceError';
  }
}

type RequestLike = { capture_id?: unknown; candidates: Array<{ id: string }> };
export type S1Choice = { choice: string | null; confidence: number; probabilities: Record<string, number> };

function isLoopback(host: string): boolean {
  const bare = host.replace(/^\[|\]$/g, '');
  if (bare === 'localhost') return true;
  if (isIP(bare) === 4) return bare.split('.')[0] === '127';
  if (isIP(bare) === 6) return bare === '::1' || /^(0{0,4}:){7}0{0,3}1$/.test(bare);
  return false;
}

/** Return the configured decide URL; it must be plain HTTP on loopback. */
export function s1ServiceUrl(value?: string): string {
  const url = (value ?? process.env[URL_ENV] ?? '').trim();
  if (!url) throw new S1ServiceError(`${URL_ENV} is not set`);
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new S1ServiceError(`${URL_ENV} must be an http:// URL on the loopback interface`);
  }
  if (parsed.protocol !== 'http:' || !isLoopback(parsed.hostname) || parsed.username || parsed.password) {
    throw new S1ServiceError(`${URL_ENV} must be an http:// URL on the loopback interface`);
  }
  return url;
}

function finiteUnit(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 1;
}

/** Check a `cua.decision_choice_v1` response against the request it answers. */
export function validateDecision(decision: unknown, request: RequestLike): Record<string, unknown> {
  if (typeof decision !== 'object' || decision === null || Array.isArray(decision)) {
    throw new S1ServiceError('response is not a cua.decision_choice_v1 decision');
  }
  const value = decision as Record<string, unknown>;
  if (value.schema !== DECISION_SCHEMA) throw new S1ServiceError('response is not a cua.decision_choice_v1 decision');
  const kind = value.kind;
  if (typeof kind !== 'string' || !DECISION_KINDS.has(kind)) throw new S1ServiceError('response kind is not supported');
  if (value.capture_id !== request.capture_id) throw new S1ServiceError('response capture_id does not match the request');
  if (kind === 'error') throw new S1ServiceError(`S1 decision failed: ${String(value.reason ?? 'unknown')}`);
  const ids = request.candidates.map(({ id }) => id);
  const selected = value.selected_id;
  if (typeof selected !== 'string' || !ids.includes(selected)) {
    throw new S1ServiceError('S1 selected an ID that was not supplied');
  }
  const expectedKind = selected === 'reobserve' || selected === 'abstain' ? selected : 'selected';
  if (kind !== expectedKind) throw new S1ServiceError('response kind does not match the selected ID');
  const probabilities = value.probabilities;
  if (
    typeof probabilities !== 'object' || probabilities === null || Array.isArray(probabilities)
    || Object.keys(probabilities).length !== ids.length
    || !ids.every((id) => finiteUnit((probabilities as Record<string, unknown>)[id]))
  ) {
    throw new S1ServiceError('response probabilities do not match the candidate set');
  }
  if (!finiteUnit(value.confidence)) throw new S1ServiceError('response confidence must be a finite number in [0, 1]');
  return value;
}

/** POST one validated request and return the chosen ID, confidence, and probabilities. */
export async function chooseS1Service(
  request: Record<string, unknown>,
  url?: string,
  timeoutMs: number = DEFAULT_TIMEOUT_MS,
): Promise<S1Choice> {
  const validated = validateRequest(request);
  let response: Response;
  try {
    response = await fetch(s1ServiceUrl(url), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (error) {
    if (error instanceof S1ServiceError) throw error;
    throw new S1ServiceError(`S1 service is unreachable: ${(error as Error).name}`);
  }
  if (!response.ok) throw new S1ServiceError(`S1 service returned HTTP ${response.status}`);
  const text = await response.text();
  if (Buffer.byteLength(text) > MAX_RESPONSE_BYTES) throw new S1ServiceError('S1 response is too large');
  let decision: unknown;
  try {
    decision = JSON.parse(text);
  } catch {
    throw new S1ServiceError('S1 response is not JSON');
  }
  const value = validateDecision(decision, validated);
  return {
    choice: value.selected_id as string,
    confidence: value.confidence as number,
    probabilities: { ...(value.probabilities as Record<string, number>) },
  };
}

import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

import { TypeSafeClient } from '@typesafe-ai/sdk';

import { chooseBoundedWithTypeSafe, type ProviderChoice } from './jev_adapter.js';

export const REQUEST_SCHEMA = 'cua.jev_choice_request_v1';
export const RESPONSE_SCHEMA = 'cua.jev_choice_v1';
const MAX_INPUT_BYTES = 65_536;
const MAX_CANDIDATES = 32;
const MAX_REGIONS = 100;
const MAX_HISTORY = 16;
const ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$/;

type JsonRecord = Record<string, unknown>;
type ValidatedRequest = {
  goal: string;
  capture_id: string;
  regions: JsonRecord[];
  history: unknown[];
  candidates: { id: string; description: string }[];
};

function record(value: unknown, message: string): JsonRecord {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(message);
  return value as JsonRecord;
}

function boundedString(value: unknown, name: string, limit: number): string {
  if (typeof value !== 'string' || !value.trim() || value.length > limit) {
    throw new Error(`${name} must be a nonempty string of at most ${limit} characters`);
  }
  return value;
}

function identifier(value: unknown, name: string): string {
  const result = boundedString(value, name, 64);
  if (!ID_PATTERN.test(result)) throw new Error(`${name} contains unsupported characters`);
  return result;
}

function exactKeys(value: JsonRecord, expected: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  const sortedExpected = [...expected].sort();
  return (
    actual.length === sortedExpected.length &&
    actual.every((key, index) => key === sortedExpected[index])
  );
}

export function validateRequest(value: unknown): ValidatedRequest {
  const root = record(value, 'request must be a JSON object');
  const rootKeys = ['candidates', 'capture_id', 'goal', 'history', 'regions', 'schema'];
  if (!exactKeys(root, rootKeys) || root.schema !== REQUEST_SCHEMA) {
    throw new Error(`request must match ${REQUEST_SCHEMA}`);
  }
  const goal = boundedString(root.goal, 'goal', 4_000);
  const captureId = boundedString(root.capture_id, 'capture_id', 256);

  if (!Array.isArray(root.regions) || root.regions.length > MAX_REGIONS) {
    throw new Error(`regions must be an array of at most ${MAX_REGIONS} items`);
  }
  const regionIds = new Set<string>();
  const regions = root.regions.map((item) => {
    const raw = record(item, 'region must be an object');
    const allowed = new Set(['id', 'kind', 'bounds', 'text', 'label', 'confidence', 'interactive']);
    if (
      Object.keys(raw).some((key) => !allowed.has(key)) ||
      !['id', 'kind', 'bounds', 'confidence', 'interactive'].every((key) => key in raw)
    ) {
      throw new Error('region has unsupported or missing fields');
    }
    const id = boundedString(raw.id, 'region id', 256);
    if (regionIds.has(id)) throw new Error('region IDs must be unique');
    regionIds.add(id);
    if (raw.kind !== 'text' && raw.kind !== 'icon') {
      throw new Error('region kind must be text or icon');
    }
    const bounds = record(raw.bounds, 'region bounds must be an object');
    if (!exactKeys(bounds, ['height', 'width', 'x', 'y'])) {
      throw new Error('region bounds have unsupported or missing fields');
    }
    for (const key of ['x', 'y', 'width', 'height'] as const) {
      const number = bounds[key];
      const minimum = key === 'x' || key === 'y' ? 0 : 1;
      if (!Number.isInteger(number) || Number(number) < minimum) {
        throw new Error('region bounds must contain valid integers');
      }
    }
    const text =
      raw.text === undefined || raw.text === null
        ? null
        : boundedString(raw.text, 'region text', 1_000);
    const label =
      raw.label === undefined || raw.label === null
        ? null
        : boundedString(raw.label, 'region label', 1_000);
    if ((raw.kind === 'text' && text === null) || (raw.kind === 'icon' && label === null)) {
      throw new Error('region is missing content required by its kind');
    }
    if (
      typeof raw.confidence !== 'number' ||
      !Number.isFinite(raw.confidence) ||
      raw.confidence < 0 ||
      raw.confidence > 1
    ) {
      throw new Error('region confidence must be between zero and one');
    }
    if (typeof raw.interactive !== 'boolean') {
      throw new Error('region interactive must be boolean');
    }
    return {
      id,
      kind: raw.kind,
      bounds: { ...bounds },
      text,
      label,
      confidence: raw.confidence,
      interactive: raw.interactive,
    };
  });

  if (!Array.isArray(root.history) || root.history.length > MAX_HISTORY) {
    throw new Error(`history must be an array of at most ${MAX_HISTORY} items`);
  }
  const history = root.history.map((item) => {
    const raw = record(item, 'history item must be an object');
    if (
      Object.keys(raw).length === 0 ||
      Object.keys(raw).some((key) => key !== 'selected_id' && key !== 'outcome')
    ) {
      throw new Error('history contains a forbidden field');
    }
    return {
      ...('selected_id' in raw
        ? { selected_id: identifier(raw.selected_id, 'history selected_id') }
        : {}),
      ...('outcome' in raw
        ? { outcome: boundedString(raw.outcome, 'history outcome', 128) }
        : {}),
    };
  });

  if (
    !Array.isArray(root.candidates) ||
    root.candidates.length < 2 ||
    root.candidates.length > MAX_CANDIDATES
  ) {
    throw new Error(`candidates must contain between 2 and ${MAX_CANDIDATES} items`);
  }
  const candidateIds = new Set<string>();
  const candidates = root.candidates.map((item) => {
    const raw = record(item, 'candidate must be an object');
    if (!exactKeys(raw, ['description', 'id'])) {
      throw new Error('candidate may contain only id and description');
    }
    const id = identifier(raw.id, 'candidate id');
    if (candidateIds.has(id)) throw new Error('candidate IDs must be unique');
    candidateIds.add(id);
    return { id, description: boundedString(raw.description, 'description', 1_000) };
  });
  if (!candidateIds.has('reobserve') || !candidateIds.has('abstain')) {
    throw new Error('candidates must include reobserve and abstain');
  }
  return { goal, capture_id: captureId, regions, history, candidates };
}

export async function chooseRequest(
  request: unknown,
  options: { client?: Pick<TypeSafeClient, 'systemOne'>; mock?: boolean } = {}
) {
  const validated = validateRequest(request);
  const criteria = Object.fromEntries(
    validated.candidates.map(({ id, description }) => [id, description])
  );
  let result: ProviderChoice;
  if (options.mock) {
    const selectedId =
      Object.keys(criteria).find((id) => id !== 'reobserve' && id !== 'abstain') ??
      'reobserve';
    result = {
      selectedId,
      model: 'mock',
      confidence: 1,
      probabilities: Object.fromEntries(
        Object.keys(criteria).map((id) => [id, Number(id === selectedId)])
      ),
    };
  } else {
    result = await chooseBoundedWithTypeSafe(
      options.client ?? new TypeSafeClient(),
      validated.goal,
      {
        capture_id: validated.capture_id,
        regions: validated.regions,
        history: validated.history,
      },
      criteria
    );
  }
  return {
    schema: RESPONSE_SCHEMA,
    selected_id: result.selectedId,
    model: result.model ?? null,
    confidence: result.confidence,
    probabilities: result.probabilities,
  };
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length > 1 || (args.length === 1 && args[0] !== '--mock')) {
    throw new Error('usage: choose_action.ts [--mock]');
  }
  const raw = readFileSync(0, 'utf8');
  if (Buffer.byteLength(raw, 'utf8') > MAX_INPUT_BYTES) {
    throw new Error('request exceeds input limit');
  }
  const response = await chooseRequest(JSON.parse(raw), { mock: args[0] === '--mock' });
  process.stdout.write(`${JSON.stringify(response)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(() => {
    process.stderr.write('chooser failed\n');
    process.exitCode = 1;
  });
}

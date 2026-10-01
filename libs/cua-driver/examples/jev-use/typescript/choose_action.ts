import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

import { TypeSafeClient } from '@typesafe-ai/sdk';

import { chooseBoundedWithTypeSafe, type ProviderChoice } from './jev_adapter.js';
import { ROLE_CLASSES, type RoleClass } from './native_roles.js';

export const REQUEST_SCHEMA = 'cua.jev_choice_request_v1';
// Additive over v1 (RFC #4268): per-candidate `source`, an optional root
// `snapshot_id`, an optional compact `elements` list of native controls, and
// an optional `progress` list of task steps counted from the runner's own
// performed actions (#4313).
export const REQUEST_SCHEMA_V2 = 'cua.jev_choice_request_v2';
export const RESPONSE_SCHEMA = 'cua.jev_choice_v1';
const MAX_INPUT_BYTES = 65_536;
const MAX_CANDIDATES = 32;
const MAX_REGIONS = 100;
const MAX_HISTORY = 16;
export const MAX_ELEMENTS = 64;
export const MAX_PROGRESS = 16;
export const MAX_PROGRESS_COUNT = 64;
const ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$/;
const RESERVED_IDS = new Set(['reobserve', 'abstain']);
export type CandidateSourceKind = 'page' | 'ax' | 'visual';
const CANDIDATE_SOURCES = new Set<string>(['page', 'ax', 'visual']);
export type ElementState =
  | 'enabled'
  | 'checked'
  | 'unchecked'
  | 'selected'
  | 'not_selected'
  | 'empty'
  | 'has_text';
const ELEMENT_STATES = new Set<string>([
  'enabled',
  'checked',
  'unchecked',
  'selected',
  'not_selected',
  'empty',
  'has_text',
]);
const V1_ROOT_KEYS = ['candidates', 'capture_id', 'goal', 'history', 'regions', 'schema'];
const V2_OPTIONAL_ROOT_KEYS = ['elements', 'progress', 'snapshot_id'];

type JsonRecord = Record<string, unknown>;
export type CompactElement = { role_class: RoleClass; label: string; state: ElementState };
export type ProgressItem = { step: string; done: number; required: number };
export type ValidatedCandidate = { id: string; description: string; source?: CandidateSourceKind };
export type ValidatedRequest = {
  schema: typeof REQUEST_SCHEMA | typeof REQUEST_SCHEMA_V2;
  goal: string;
  capture_id: string;
  regions: JsonRecord[];
  history: unknown[];
  candidates: ValidatedCandidate[];
  snapshot_id?: string | null;
  elements?: CompactElement[];
  progress?: ProgressItem[];
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

function validateElements(value: unknown): CompactElement[] {
  if (!Array.isArray(value) || value.length > MAX_ELEMENTS) {
    throw new Error(`elements must be an array of at most ${MAX_ELEMENTS} items`);
  }
  return value.map((item) => {
    const raw = record(item, 'element must be an object');
    if (!exactKeys(raw, ['label', 'role_class', 'state'])) {
      throw new Error('element may contain only role_class, label, and state');
    }
    if (typeof raw.role_class !== 'string' || !(ROLE_CLASSES as readonly string[]).includes(raw.role_class)) {
      throw new Error('element role_class is not a supported role class');
    }
    if (typeof raw.state !== 'string' || !ELEMENT_STATES.has(raw.state)) {
      throw new Error('element state is not a supported state');
    }
    return {
      role_class: raw.role_class as RoleClass,
      label: boundedString(raw.label, 'element label', 200),
      state: raw.state as ElementState,
    };
  });
}

function count(value: unknown, name: string, minimum: number): number {
  if (!Number.isInteger(value) || Number(value) < minimum || Number(value) > MAX_PROGRESS_COUNT) {
    throw new Error(`${name} must be an integer from ${minimum} to ${MAX_PROGRESS_COUNT}`);
  }
  return Number(value);
}

/**
 * Validate task steps and how often this run has performed each one. `done`
 * is counted from the runner's own performed actions, never read from the
 * application, so progress carries no application values.
 */
function validateProgress(value: unknown): ProgressItem[] {
  if (!Array.isArray(value) || value.length > MAX_PROGRESS) {
    throw new Error(`progress must be an array of at most ${MAX_PROGRESS} items`);
  }
  return value.map((item) => {
    const raw = record(item, 'progress item must be an object');
    if (!exactKeys(raw, ['done', 'required', 'step'])) {
      throw new Error('progress item may contain only step, done, and required');
    }
    return {
      step: boundedString(raw.step, 'progress step', 200),
      done: count(raw.done, 'progress done', 0),
      required: count(raw.required, 'progress required', 1),
    };
  });
}

/**
 * Validate a cua.jev_choice_request_v1 or _v2 request strictly. v1 is
 * unchanged, so any v2 field in a v1 request is rejected. v2 may add a root
 * snapshot_id, elements, and progress and a per-candidate source; reserved
 * candidates never carry a source.
 */
export function validateRequest(value: unknown): ValidatedRequest {
  const root = record(value, 'request must be a JSON object');
  const keys = Object.keys(root);
  let v2 = false;
  if (root.schema === REQUEST_SCHEMA) {
    if (!exactKeys(root, V1_ROOT_KEYS)) throw new Error(`request must match ${REQUEST_SCHEMA}`);
  } else if (root.schema === REQUEST_SCHEMA_V2) {
    v2 = true;
    if (
      !V1_ROOT_KEYS.every((key) => key in root) ||
      keys.some((key) => !V1_ROOT_KEYS.includes(key) && !V2_OPTIONAL_ROOT_KEYS.includes(key))
    ) {
      throw new Error(`request must match ${REQUEST_SCHEMA_V2}`);
    }
  } else {
    throw new Error(`request must match ${REQUEST_SCHEMA} or ${REQUEST_SCHEMA_V2}`);
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
  const candidates = root.candidates.map((item): ValidatedCandidate => {
    const raw = record(item, 'candidate must be an object');
    if (v2) {
      const candidateKeys = Object.keys(raw);
      if (
        !('id' in raw) ||
        !('description' in raw) ||
        candidateKeys.some((key) => key !== 'id' && key !== 'description' && key !== 'source')
      ) {
        throw new Error('candidate may contain only id, description, and source');
      }
    } else if (!exactKeys(raw, ['description', 'id'])) {
      throw new Error('candidate may contain only id and description');
    }
    const id = identifier(raw.id, 'candidate id');
    if (candidateIds.has(id)) throw new Error('candidate IDs must be unique');
    candidateIds.add(id);
    const candidate: ValidatedCandidate = {
      id,
      description: boundedString(raw.description, 'description', 1_000),
    };
    if ('source' in raw) {
      if (RESERVED_IDS.has(id)) throw new Error('reserved candidates must not carry a source');
      if (typeof raw.source !== 'string' || !CANDIDATE_SOURCES.has(raw.source)) {
        throw new Error('candidate source must be page, ax, or visual');
      }
      candidate.source = raw.source as CandidateSourceKind;
    }
    return candidate;
  });
  if (!candidateIds.has('reobserve') || !candidateIds.has('abstain')) {
    throw new Error('candidates must include reobserve and abstain');
  }
  const validated: ValidatedRequest = {
    schema: v2 ? REQUEST_SCHEMA_V2 : REQUEST_SCHEMA,
    goal,
    capture_id: captureId,
    regions,
    history,
    candidates,
  };
  if (v2) {
    validated.snapshot_id =
      root.snapshot_id === undefined || root.snapshot_id === null
        ? null
        : boundedString(root.snapshot_id, 'snapshot_id', 64);
    validated.elements = validateElements(root.elements ?? []);
    validated.progress = validateProgress(root.progress ?? []);
  }
  return validated;
}

/**
 * The observation a provider sees. A v1 request keeps its exact v1
 * observation; v2 adds the snapshot, compact elements, and candidate sources,
 * plus progress when the request carries any.
 */
export function providerObservation(validated: ValidatedRequest): JsonRecord {
  const observation: JsonRecord = {
    capture_id: validated.capture_id,
    regions: validated.regions,
    history: validated.history,
  };
  if (validated.schema === REQUEST_SCHEMA_V2) {
    observation.snapshot_id = validated.snapshot_id ?? null;
    observation.elements = validated.elements ?? [];
    observation.candidate_sources = Object.fromEntries(
      validated.candidates.filter((item) => item.source).map((item) => [item.id, item.source])
    );
    if (validated.progress?.length) observation.progress = validated.progress;
  }
  return observation;
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
      providerObservation(validated),
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

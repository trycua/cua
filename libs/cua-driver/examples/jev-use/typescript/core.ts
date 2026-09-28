/**
 * Runner primitives shared by every jev-use task: visual-region parsing,
 * candidate validation, and the mock chooser. Candidate sources are in
 * sources.ts and task specs in tasks.ts; the names re-exported below keep
 * existing imports from core.ts working.
 */
import type { Candidate } from './sources.js';

export type { Candidate } from './sources.js';
export {
  FIELD_NAME,
  HISTORY_OUTCOMES,
  REDACTED_TOKEN,
  SUBMIT_IDS,
  SUBMIT_NAME,
  buildCandidates,
  classify,
  formState,
  historyEntry,
  redactToken,
  visualSubmitRegion,
  type FormState,
  type HistoryEntry,
  type Outcome,
  type SubmitButtonState,
} from './tasks.js';

export type VisualDelivery = 'background' | 'foreground';

export type PageRef = {
  role?: string;
  name?: string | null;
  ref?: string;
  value?: string | null;
};

export type BrowserSnapshot = {
  target_id: string;
  tab_id: string;
  capture_id?: string;
  refs?: PageRef[];
  page?: unknown;
  outline?: string;
};

export type VisualRegion = Readonly<{
  id: string;
  kind: 'text' | 'icon';
  text?: string;
  label?: string;
  confidence: number;
  interactive: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
}>;

export type VisualObservation = Readonly<{
  captureId: string;
  screenshotReference: string;
  screenshotWidth: number;
  screenshotHeight: number;
  pid: number;
  windowId: number;
  actionOriginX: number;
  actionOriginY: number;
  actionUnitsPerPixelX: number;
  actionUnitsPerPixelY: number;
  regions: readonly VisualRegion[];
}>;

export class VisualObservationError extends Error {
  constructor(
    message: string,
    readonly code: string = 'invalid_visual_result'
  ) {
    super(message);
    this.name = 'VisualObservationError';
  }
}

function record(value: unknown, message: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(message);
  return value as Record<string, unknown>;
}

function nonempty(value: unknown): string {
  if (typeof value !== 'string' || !value.trim()) {
    throw new Error('visual result contains an empty string');
  }
  return value;
}

function positiveInt(value: unknown): number {
  if (!Number.isInteger(value) || Number(value) <= 0) {
    throw new Error('visual result contains an invalid positive integer');
  }
  return Number(value);
}

function pixelInt(value: unknown): number {
  if (!Number.isInteger(value) || Number(value) < 0) {
    throw new Error('visual result contains an invalid pixel coordinate');
  }
  return Number(value);
}

export function parseVisualRegions(
  payload: unknown,
  expectedCaptureId: string,
  expectedPid: number,
  expectedWindowId: number
): VisualObservation {
  const root = record(payload, 'visual result is not an object');
  if (root.schema !== 'cua.visual_regions_v1') throw new Error('unsupported visual region schema');
  const capture = record(root.capture, 'visual result has no capture provenance');
  if (capture.capture_id !== expectedCaptureId) {
    throw new VisualObservationError(
      'visual result is stale or capture-mismatched',
      'capture_mismatch'
    );
  }
  const source = record(capture.source, 'visual result has no capture source');
  if (
    source.kind !== 'window' ||
    source.pid !== expectedPid ||
    source.window_id !== expectedWindowId
  ) {
    throw new VisualObservationError(
      'visual result has a mismatched window target',
      'capture_mismatch'
    );
  }
  const screenshot = record(capture.screenshot, 'visual result has no screenshot provenance');
  if (screenshot.mime_type !== 'image/png') {
    throw new Error('visual result has invalid screenshot provenance');
  }
  const screenshotReference = nonempty(screenshot.reference);
  const screenshotWidth = positiveInt(screenshot.width);
  const screenshotHeight = positiveInt(screenshot.height);

  const space = record(capture.action_coordinate_space, 'visual result has no coordinate mapping');
  let actionOriginX = 0;
  let actionOriginY = 0;
  let actionUnitsPerPixelX = 1;
  let actionUnitsPerPixelY = 1;
  if (space.kind === 'scaled_top_left') {
    const values = [
      space.action_origin_x,
      space.action_origin_y,
      space.action_units_per_pixel_x,
      space.action_units_per_pixel_y,
    ];
    if (values.some((value) => typeof value !== 'number' || !Number.isFinite(value))) {
      throw new Error('visual result has malformed coordinate mapping');
    }
    [actionOriginX, actionOriginY, actionUnitsPerPixelX, actionUnitsPerPixelY] = values as number[];
    if (actionUnitsPerPixelX <= 0 || actionUnitsPerPixelY <= 0) {
      throw new Error('visual result has non-positive coordinate scale');
    }
  } else if (space.kind !== 'screenshot_pixels') {
    throw new Error('visual result has unsupported coordinate mapping');
  }

  if (!Array.isArray(root.regions)) throw new Error('visual result has no region list');
  const ids = new Set<string>();
  const regions = root.regions.map((item) => {
    const raw = record(item, 'visual result contains a malformed region');
    const id = nonempty(raw.id);
    if (ids.has(id)) throw new Error('visual result contains duplicate region IDs');
    ids.add(id);
    if (raw.kind !== 'text' && raw.kind !== 'icon') {
      throw new Error('visual result contains an unsupported region kind');
    }
    const bounds = record(raw.bounds, 'visual result contains malformed bounds');
    const x = pixelInt(bounds.x);
    const y = pixelInt(bounds.y);
    const width = positiveInt(bounds.width);
    const height = positiveInt(bounds.height);
    if (x + width > screenshotWidth || y + height > screenshotHeight) {
      throw new Error('visual region is outside its source screenshot');
    }
    if (typeof raw.confidence !== 'number' || !Number.isFinite(raw.confidence) || raw.confidence < 0 || raw.confidence > 1) {
      throw new Error('visual result contains invalid confidence');
    }
    const text = raw.text === undefined || raw.text === null ? undefined : nonempty(raw.text);
    const label = raw.label === undefined || raw.label === null ? undefined : nonempty(raw.label);
    if ((raw.kind === 'text' && text === undefined) || (raw.kind === 'icon' && label === undefined)) {
      throw new Error('visual region is missing content required by its kind');
    }
    if (typeof raw.interactive !== 'boolean') {
      throw new Error('visual region has malformed interactivity');
    }
    return Object.freeze({
      id,
      kind: raw.kind,
      text,
      label,
      confidence: raw.confidence,
      interactive: raw.interactive,
      x,
      y,
      width,
      height,
    });
  });

  return Object.freeze({
    captureId: expectedCaptureId,
    screenshotReference,
    screenshotWidth,
    screenshotHeight,
    pid: expectedPid,
    windowId: expectedWindowId,
    actionOriginX,
    actionOriginY,
    actionUnitsPerPixelX,
    actionUnitsPerPixelY,
    regions: Object.freeze(regions),
  });
}

export function hasExecutableCandidate(candidates: readonly Candidate[]): boolean {
  return candidates.some((candidate) => candidate.tool !== null);
}

export function chooseMock(candidates: Candidate[]) {
  const ids = new Set(candidates.map((candidate) => candidate.id));
  const selected = ids.has('type-verification-value')
    ? 'type-verification-value'
    : ids.has('submit-form')
      ? 'submit-form'
      : ids.has('submit-form-foreground')
        ? 'submit-form-foreground'
        : ids.has('reobserve')
          ? 'reobserve'
          : null;
  return {
    choice: selected,
    confidence: selected ? 1 : 0,
    probabilities: Object.fromEntries(
      candidates.map((candidate) => [candidate.id, Number(candidate.id === selected)])
    ),
  };
}

export function validateChoice(
  choice: string,
  candidates: Candidate[],
  currentCaptureId?: string
): Candidate {
  if (typeof choice !== 'string' || !choice) throw new Error('provider selected a malformed candidate ID');
  const ids = candidates.map((candidate) => candidate.id);
  if (new Set(ids).size !== ids.length) throw new Error('candidate set contains duplicate IDs');
  const candidate = candidates.find((item) => item.id === choice);
  if (!candidate) throw new Error(`provider selected unknown candidate: ${choice}`);
  if (candidate.captureId !== undefined && candidate.captureId !== currentCaptureId) {
    throw new Error('provider selected a stale or capture-mismatched candidate');
  }
  return candidate;
}

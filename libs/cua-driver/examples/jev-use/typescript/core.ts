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
  /**
   * Driver's screenshot-to-action affine `[m11, m12, m21, m22, tx, ty]`. It is
   * validated but never applied here: a capture-bound click sends the original
   * screenshot point and `capture_id`, and Driver maps it once.
   */
  screenshotToAction: AffineCoefficients;
  regions: readonly VisualRegion[];
}>;

export type AffineCoefficients = readonly [number, number, number, number, number, number];

export const IDENTITY_MAPPING: AffineCoefficients = Object.freeze([1, 0, 0, 1, 0, 0] as const);

/**
 * Return Driver's screenshot-to-action affine from `parse_visual_regions`.
 *
 * Driver reports `screenshot_pixels` for an identity transform and `affine`
 * otherwise (for example, a Retina window capture). The mapping must be finite
 * and invertible, matching Driver's own contract validation. Mirrors
 * `action_coordinate_mapping` in python/action_policy.py.
 */
export function actionCoordinateMapping(space: unknown): AffineCoefficients {
  if (!space || typeof space !== 'object' || Array.isArray(space)) {
    throw new Error('unsupported action coordinate space');
  }
  const value = space as Record<string, unknown>;
  if (value.kind === 'screenshot_pixels') return IDENTITY_MAPPING;
  if (value.kind !== 'affine') throw new Error('unsupported action coordinate space');
  const coefficients = (['m11', 'm12', 'm21', 'm22', 'tx', 'ty'] as const).map((key) => value[key]);
  if (coefficients.some((item) => typeof item !== 'number' || !Number.isFinite(item))) {
    throw new Error('malformed action coordinate mapping');
  }
  const [m11, m12, m21, m22, tx, ty] = coefficients as number[];
  if (Math.abs(m11 * m22 - m12 * m21) <= Number.EPSILON) {
    throw new Error('malformed action coordinate mapping');
  }
  return Object.freeze([m11, m12, m21, m22, tx, ty] as const);
}

/** Map one screenshot point, refusing a mapping that overflows it. */
export function mapScreenshotPoint(
  mapping: AffineCoefficients,
  x: number,
  y: number
): readonly [number, number] {
  const [m11, m12, m21, m22, tx, ty] = mapping;
  const actionX = m11 * x + m12 * y + tx;
  const actionY = m21 * x + m22 * y + ty;
  if (!Number.isFinite(actionX) || !Number.isFinite(actionY)) {
    throw new Error('action coordinate mapping produced a non-finite point');
  }
  return [actionX, actionY];
}

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

  let screenshotToAction: AffineCoefficients;
  try {
    screenshotToAction = actionCoordinateMapping(capture.action_coordinate_space);
    for (const [x, y] of [
      [0, 0],
      [screenshotWidth, 0],
      [0, screenshotHeight],
      [screenshotWidth, screenshotHeight],
    ]) {
      mapScreenshotPoint(screenshotToAction, x, y);
    }
  } catch (error) {
    throw new Error(`visual result has ${(error as Error).message}`);
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
    screenshotToAction,
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

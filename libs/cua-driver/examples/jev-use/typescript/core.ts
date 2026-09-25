export type Outcome = 'verified' | 'refuted' | 'unknown' | 'abstained' | 'budget_exhausted';

export type Candidate = Readonly<{
  id: string;
  description: string;
  tool: string | null;
  arguments: Readonly<Record<string, unknown>>;
  captureId?: string;
  screenshotReference?: string;
}>;

type PageRef = {
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

function immutableCandidate(candidate: Candidate): Candidate {
  return Object.freeze({ ...candidate, arguments: Object.freeze({ ...candidate.arguments }) });
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

function reservedCandidates(): Candidate[] {
  return [
    immutableCandidate({
      id: 'reobserve',
      description: 'Discard this decision set and obtain a fresh Driver observation.',
      tool: null,
      arguments: {},
    }),
    immutableCandidate({
      id: 'abstain',
      description: 'Stop without acting if none of the proposed actions is safe for the observed state.',
      tool: null,
      arguments: {},
    }),
  ];
}

export function buildCandidates(
  snapshot: BrowserSnapshot,
  token: string,
  visual?: VisualObservation,
  captureBoundClick = false
): Candidate[] {
  const common = { target_id: snapshot.target_id, tab_id: snapshot.tab_id };
  const refs = snapshot.refs ?? [];
  const field = refs.find(
    (item) => item.role === 'textbox' && item.name === 'verification value' && item.ref
  );
  const button = refs.find((item) => item.role === 'button' && item.name === 'Submit' && item.ref);
  const candidates: Candidate[] = [];
  if (field?.value !== token && field?.ref) {
    candidates.push(
      immutableCandidate({
        id: 'type-verification-value',
        description: 'Replace the verification field with the required token.',
        tool: 'browser_type',
        arguments: { ...common, ref: field.ref, text: token, replace: true },
      })
    );
  } else if (field?.value === token && button?.ref) {
    candidates.push(
      immutableCandidate({
        id: 'submit-form',
        description: 'Submit the form now that the verification field contains the token.',
        tool: 'browser_click',
        arguments: { ...common, ref: button.ref, input_route: 'dom_event' },
      })
    );
  } else if (
    field?.value === token &&
    visual &&
    captureBoundClick
  ) {
    const matches = visual.regions.filter(
      (region) =>
        region.confidence >= 0.8 &&
        asciiLower(region.text ?? region.label ?? '') === 'submit'
    );
    if (matches.length === 1) {
      const region = matches[0];
      const x = region.x + region.width / 2;
      const y = region.y + region.height / 2;
      candidates.push(
        immutableCandidate({
          id: 'submit-form',
          description: 'Submit the form using the unique validated visual Submit region.',
          tool: 'click',
          arguments: {
            pid: visual.pid,
            window_id: visual.windowId,
            x,
            y,
            capture_id: visual.captureId,
            delivery_mode: 'background',
          },
          captureId: visual.captureId,
          screenshotReference: visual.screenshotReference,
        })
      );
    }
  }
  return [...candidates, ...reservedCandidates()];
}

function asciiLower(value: string): string {
  return value.replace(/[A-Z]/g, (character) =>
    String.fromCharCode(character.charCodeAt(0) + 32)
  );
}

export function chooseMock(candidates: Candidate[]) {
  const ids = new Set(candidates.map((candidate) => candidate.id));
  const selected = ids.has('type-verification-value')
    ? 'type-verification-value'
    : ids.has('submit-form')
      ? 'submit-form'
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

export function classify(
  submitted: string | null,
  token: string,
  steps: number,
  maxSteps: number
): Outcome {
  if (submitted === token) return 'verified';
  if (submitted !== null) return 'refuted';
  if (steps >= maxSteps) return 'budget_exhausted';
  return 'unknown';
}

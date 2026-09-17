import type { Candidate } from './core.js';

type Bounds = { x: number; y: number; width: number; height: number };
type Observation = {
  source: { kind: string; pid?: number; window_id?: number };
  snapshot_id: string;
  image_width: number;
  image_height: number;
  semantic_elements?: { name?: string; bounds?: Bounds }[];
};

function validBox(value: unknown, width: number, height: number): value is Bounds {
  if (!value || typeof value !== 'object') return false;
  const box = value as Record<string, unknown>;
  const values = ['x', 'y', 'width', 'height'].map((key) => box[key]);
  if (!values.every(Number.isInteger)) return false;
  const [x, y, boxWidth, boxHeight] = values as number[];
  return x >= 0 && y >= 0 && boxWidth > 0 && boxHeight > 0 && x + boxWidth <= width && y + boxHeight <= height;
}

function overlaps(left: Bounds, right: Bounds): boolean {
  return left.x < right.x + right.width && right.x < left.x + left.width && left.y < right.y + right.height && right.y < left.y + left.height;
}

export function buildVisualCandidates(
  observation: Observation,
  response: Record<string, any>,
  minimumConfidence = 0.8
): Candidate[] {
  const fallback: Candidate[] = [
    { id: 'reobserve', description: 'Capture fresh evidence before proposing another action.', tool: null, arguments: {} },
    { id: 'abstain', description: 'Stop without acting when the evidence is unsafe or ambiguous.', tool: null, arguments: {} },
  ];
  const capture = response.capture ?? {};
  if (
    response.schema !== 'cua.visual_regions_v1' ||
    JSON.stringify(capture.source) !== JSON.stringify(observation.source) ||
    capture.snapshot_id !== observation.snapshot_id ||
    capture.image_width !== observation.image_width ||
    capture.image_height !== observation.image_height ||
    observation.source.kind !== 'window' ||
    !Number.isInteger(observation.source.pid) ||
    !Number.isInteger(observation.source.window_id)
  ) return fallback;

  const matches = (response.regions ?? []).filter((region: Record<string, any>) => {
    const box = region.bounds;
    const center = region.center ?? {};
    if (
      region.interactive !== true ||
      typeof region.confidence !== 'number' ||
      region.confidence < minimumConfidence ||
      !validBox(box, observation.image_width, observation.image_height) ||
      !Number.isInteger(center.x) ||
      !Number.isInteger(center.y) ||
      center.x < box.x || center.x >= box.x + box.width ||
      center.y < box.y || center.y >= box.y + box.height
    ) return false;
    const aligned = (observation.semantic_elements ?? []).filter(
      (element) =>
        element.name?.toLocaleLowerCase() === String(region.text ?? '').toLocaleLowerCase() &&
        validBox(element.bounds, observation.image_width, observation.image_height) &&
        overlaps(box, element.bounds)
    );
    return aligned.length === 1;
  });
  if (matches.length !== 1) return fallback;
  const region = matches[0];
  return [
    {
      id: 'click-visual-save',
      description: 'Click the fresh visual region labelled Save.',
      tool: 'click',
      arguments: {
        pid: observation.source.pid,
        window_id: observation.source.window_id,
        x: region.center.x,
        y: region.center.y,
        delivery_mode: 'background',
      },
    },
    ...fallback,
  ];
}

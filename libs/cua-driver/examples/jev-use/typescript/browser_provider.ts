/**
 * Browser-runner provider choices and the backend each one reports.
 * Candidate construction, action arguments, freshness and verification remain
 * caller-owned. `live` keeps the TypeSafe request pinned by the provider-request
 * golden files; `s1` receives only the validated closed-choice request.
 */
import { REQUEST_SCHEMA, validateRequest, type ValidatedRequest } from './choose_action.js';
import type { Candidate } from './sources.js';
import type { HistoryEntry, Task, TaskSources } from './tasks.js';

export const BACKENDS = { mock: 'mock', live: 'typesafe', s1: 's1' } as const;

export type BrowserProvider = keyof typeof BACKENDS;
export type Backend = (typeof BACKENDS)[BrowserProvider];

export function isBrowserProvider(value: string): value is BrowserProvider {
  return Object.hasOwn(BACKENDS, value);
}

export function browserDecisionRequest(
  task: Task,
  sources: TaskSources,
  candidates: readonly Candidate[],
  history: readonly HistoryEntry[]
): ValidatedRequest {
  const page = sources.page?.snapshot;
  if (!page) throw new Error('browser provider requires page sources');
  const visual = sources.visual?.observation;
  const captureId =
    visual?.captureId ??
    (typeof page.capture_id === 'string' && page.capture_id
      ? page.capture_id
      : `browser:${page.target_id}:${page.tab_id}`);
  const regions =
    visual?.regions.map((region) => ({
      id: region.id,
      kind: region.kind,
      bounds: {
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
      },
      ...(region.text ? { text: region.text } : {}),
      ...(region.label ? { label: region.label } : {}),
      confidence: region.confidence,
      interactive: region.interactive,
    })) ?? [];
  return validateRequest({
    schema: REQUEST_SCHEMA,
    goal: task.goal,
    capture_id: captureId,
    regions: task.redact(regions),
    history: history.map(({ selected_id, outcome }) => ({ selected_id, outcome })),
    candidates: candidates.map(({ id, description }) => ({ id, description })),
  });
}

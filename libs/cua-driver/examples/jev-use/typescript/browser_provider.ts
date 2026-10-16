/**
 * Browser-runner provider parity on the existing bounded decision seam.
 * Candidate construction, action arguments, freshness and verification remain
 * caller-owned. Providers receive only supplied candidate IDs/descriptions.
 */
import { TypeSafeClient } from '@typesafe-ai/sdk';

import {
  REQUEST_SCHEMA,
  providerObservation,
  validateRequest,
  type ValidatedRequest,
} from './choose_action.js';
import { chooseBoundedWithTypeSafe, chooseMockForTask } from './jev_adapter.js';
import { chooseS1Service } from './s1_service.js';
import type { Candidate } from './sources.js';
import type { HistoryEntry, Task, TaskSources } from './tasks.js';

export type BrowserProvider = 'mock' | 'live' | 'typesafe' | 's1';

export type BrowserDecision = Readonly<{
  choice: string | null;
  confidence: number;
  probabilities: Readonly<Record<string, number>>;
  backend: 'mock' | 'typesafe' | 's1';
}>;

export function backendName(
  provider: BrowserProvider
): BrowserDecision['backend'] {
  return provider === 'live' || provider === 'typesafe' ? 'typesafe' : provider;
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
  const request = {
    schema: REQUEST_SCHEMA,
    goal: task.goal,
    capture_id: captureId,
    regions: task.redact(regions),
    history: history.map(({ selected_id, outcome }) => ({ selected_id, outcome })),
    candidates: candidates.map(({ id, description }) => ({ id, description })),
  };
  return validateRequest(request);
}

export async function chooseBrowserProvider(
  provider: BrowserProvider,
  task: Task,
  sources: TaskSources,
  candidates: Candidate[],
  history: HistoryEntry[]
): Promise<BrowserDecision> {
  const backend = backendName(provider);
  if (provider === 'mock') {
    const result = chooseMockForTask(task, sources, candidates, history);
    return { ...result, backend };
  }

  const request = browserDecisionRequest(task, sources, candidates, history);
  if (provider === 's1') {
    return { ...(await chooseS1Service(request)), backend };
  }
  const criteria = Object.fromEntries(
    request.candidates.map(({ id, description }) => [id, description])
  );
  const result = await chooseBoundedWithTypeSafe(
    new TypeSafeClient(),
    request.goal,
    providerObservation(request),
    criteria
  );
  return {
    choice: result.selectedId,
    confidence: result.confidence,
    probabilities: result.probabilities,
    backend,
  };
}
import { choice, TypeSafeClient } from '@typesafe-ai/sdk';

import {
  chooseMock,
  type BrowserSnapshot,
  type HistoryEntry,
  type Candidate,
  type VisualObservation,
} from './core.js';
import {
  FIXTURE_GOAL,
  FixtureFormTask,
  fixtureSources,
  requirePage,
  type Task,
  type TaskSources,
} from './tasks.js';

type TypeSafeClientLike = Pick<TypeSafeClient, 'systemOne'>;

export type ProviderChoice = Readonly<{
  selectedId: string;
  confidence: number;
  probabilities: Readonly<Record<string, number>>;
  model?: string;
}>;

export async function chooseBoundedWithTypeSafe(
  client: TypeSafeClientLike,
  goal: string,
  observation: Readonly<Record<string, unknown>>,
  criteria: Readonly<Record<string, string>>
): Promise<ProviderChoice> {
  const response = await client.systemOne({
    state: {
      observation: JSON.stringify(observation),
    },
    questions: {
      candidate: choice(goal, { ...criteria }),
    },
  });
  const answer = response.answers.candidate;
  if (answer.type !== 'choice') throw new Error('Jev returned the wrong answer type');
  if (!Object.hasOwn(criteria, answer.choice)) {
    throw new Error(`Jev selected unknown candidate: ${answer.choice}`);
  }
  if (!Number.isFinite(answer.confidence) || answer.confidence < 0 || answer.confidence > 1) {
    throw new Error('Jev returned invalid confidence');
  }
  const probabilities: Record<string, number> = {};
  for (const [candidateId, probability] of Object.entries(answer.probabilities)) {
    if (!Object.hasOwn(criteria, candidateId)) {
      throw new Error(`Jev returned probability for unknown candidate: ${candidateId}`);
    }
    if (!Number.isFinite(probability) || probability < 0 || probability > 1) {
      throw new Error('Jev returned invalid probability');
    }
    probabilities[candidateId] = probability;
  }
  const model =
    typeof response.model === 'string' && response.model.trim() ? response.model : undefined;
  return Object.freeze({
    selectedId: answer.choice,
    confidence: answer.confidence,
    probabilities: Object.freeze(probabilities),
    ...(model ? { model } : {}),
  });
}

function candidateCriteria(candidates: Candidate[]): Record<string, string> {
  const criteria = Object.fromEntries(
    candidates.map((candidate) => [candidate.id, candidate.description])
  );
  if (Object.keys(criteria).length !== candidates.length) {
    throw new Error('candidate set contains duplicate IDs');
  }
  return criteria;
}

export function visualDecisionState(visual?: VisualObservation) {
  if (!visual) return null;
  return {
    schema: 'cua.visual_regions_v1' as const,
    capture_id: visual.captureId,
    screenshot_reference: visual.screenshotReference,
    source: {
      kind: 'window' as const,
      pid: visual.pid,
      window_id: visual.windowId,
    },
    regions: visual.regions.map((region) => ({
      id: region.id,
      kind: region.kind,
      text: region.text ?? null,
      label: region.label ?? null,
      confidence: region.confidence,
      interactive: region.interactive,
      bounds: {
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
      },
    })),
  };
}

export const GOAL = FIXTURE_GOAL;

/**
 * Build the compact, deterministic, secret-redacted state sent to Jev. `form`
 * is the task's state summary, which states what the runner verified from its
 * candidate sources, so the model does not have to infer it from the outline.
 * Every secret task parameter is replaced everywhere, including outline and
 * visual text.
 */
export function taskDecisionState(
  task: Task,
  sources: TaskSources,
  history: readonly HistoryEntry[]
) {
  const snapshot = requirePage(sources).snapshot;
  return {
    goal: task.goal,
    observation: {
      page: JSON.stringify(task.redact(snapshot.page ?? null)),
      form: JSON.stringify(task.stateSummary(sources)),
      outline: task.redact(snapshot.outline ?? '') as string,
      visual: JSON.stringify(task.redact(visualDecisionState(sources.visual?.observation))),
    },
    history: JSON.stringify(history),
  };
}

/** Build the fixture task's decision state; see taskDecisionState. */
export function decisionState(
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: readonly HistoryEntry[],
  token: string,
  visualPath = false
) {
  return taskDecisionState(
    new FixtureFormTask(token),
    fixtureSources(snapshot, visual, false, 'background', visualPath),
    history
  );
}

export const DRIVER_ACTION_INSTRUCTIONS =
  'Which complete executable action should Cua Driver run next?';

export async function chooseForTask(
  client: TypeSafeClientLike,
  task: Task,
  sources: TaskSources,
  candidates: Candidate[],
  history: readonly HistoryEntry[]
) {
  const criteria = candidateCriteria(candidates);
  const response = await client.systemOne({
    state: taskDecisionState(task, sources, history),
    questions: {
      driver_action: choice(DRIVER_ACTION_INSTRUCTIONS, criteria),
    },
  });
  const answer = response.answers.driver_action;
  if (answer.type !== 'choice') throw new Error('Jev returned the wrong answer type');
  if (!Object.hasOwn(criteria, answer.choice)) {
    throw new Error(`Jev selected unknown candidate: ${answer.choice}`);
  }
  return answer;
}

export function chooseWithTypeSafe(
  client: TypeSafeClientLike,
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: readonly HistoryEntry[],
  token: string,
  visualPath = false
) {
  return chooseForTask(
    client,
    new FixtureFormTask(token),
    fixtureSources(snapshot, visual, false, 'background', visualPath),
    candidates,
    history
  );
}

export function chooseLiveForTask(
  task: Task,
  sources: TaskSources,
  candidates: Candidate[],
  history: readonly HistoryEntry[]
) {
  return chooseForTask(new TypeSafeClient(), task, sources, candidates, history);
}

/**
 * Deterministic mock provider. A task may declare mockPreferences: the first
 * preferred ID present wins (a refused control's `<id>:foreground` variant
 * counts as its ID), otherwise reobserve. Tasks without preferences keep the
 * fixed browser-fixture order.
 */
export function chooseMockForTask(
  task: Task,
  _sources: TaskSources,
  candidates: Candidate[],
  _history: readonly HistoryEntry[]
) {
  const preferences = (task as { mockPreferences?: readonly string[] }).mockPreferences ?? [];
  if (!preferences.length) return chooseMock(candidates);
  const ids = candidates.map((candidate) => candidate.id);
  const selected =
    preferences
      .flatMap((preferred) => [preferred, `${preferred}:foreground`])
      .find((id) => ids.includes(id)) ?? (ids.includes('reobserve') ? 'reobserve' : null);
  return {
    choice: selected,
    confidence: selected ? 1 : 0,
    probabilities: Object.fromEntries(ids.map((id) => [id, Number(id === selected)])),
  };
}

export function chooseLive(
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: readonly HistoryEntry[],
  token: string,
  visualPath = false
) {
  return chooseLiveForTask(
    new FixtureFormTask(token),
    fixtureSources(snapshot, visual, false, 'background', visualPath),
    candidates,
    history
  );
}

export function chooseMockAdapter(
  candidates: Candidate[],
  _snapshot: BrowserSnapshot,
  _visual: VisualObservation | undefined,
  _history: readonly HistoryEntry[],
  _token: string,
  _visualPath = false
) {
  return chooseMock(candidates);
}

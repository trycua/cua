import { choice, TypeSafeClient } from '@typesafe-ai/sdk';

import {
  chooseMock,
  formState,
  redactToken,
  type BrowserSnapshot,
  type HistoryEntry,
  type Candidate,
  type VisualObservation,
} from './core.js';

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

export const GOAL =
  'Enter the required verification token into the verification field, then submit the form.';

/**
 * Build the compact, deterministic, token-redacted state sent to Jev. `form`
 * states the field and Submit status the runner verified from the page
 * structure; the token is replaced everywhere, including outline and visual text.
 */
export function decisionState(
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: readonly HistoryEntry[],
  token: string
) {
  return {
    goal: GOAL,
    observation: {
      page: JSON.stringify(redactToken(snapshot.page ?? null, token)),
      form: JSON.stringify(formState(snapshot, token)),
      outline: redactToken(snapshot.outline ?? '', token) as string,
      visual: JSON.stringify(redactToken(visualDecisionState(visual), token)),
    },
    history: JSON.stringify(history),
  };
}

export async function chooseWithTypeSafe(
  client: TypeSafeClientLike,
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: readonly HistoryEntry[],
  token: string
) {
  const criteria = candidateCriteria(candidates);
  const response = await client.systemOne({
    state: decisionState(snapshot, visual, history, token),
    questions: {
      driver_action: choice(
        'Which complete executable action should Cua Driver run next?',
        criteria
      ),
    },
  });
  const answer = response.answers.driver_action;
  if (answer.type !== 'choice') throw new Error('Jev returned the wrong answer type');
  if (!Object.hasOwn(criteria, answer.choice)) {
    throw new Error(`Jev selected unknown candidate: ${answer.choice}`);
  }
  return answer;
}

export function chooseLive(
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: readonly HistoryEntry[],
  token: string
) {
  return chooseWithTypeSafe(new TypeSafeClient(), candidates, snapshot, visual, history, token);
}

export function chooseMockAdapter(
  candidates: Candidate[],
  _snapshot: BrowserSnapshot,
  _visual: VisualObservation | undefined,
  _history: readonly HistoryEntry[],
  _token: string
) {
  return chooseMock(candidates);
}

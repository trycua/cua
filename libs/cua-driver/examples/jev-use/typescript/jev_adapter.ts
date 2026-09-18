import { choice, TypeSafeClient } from '@typesafe-ai/sdk';

import {
  chooseMock,
  type BrowserSnapshot,
  type Candidate,
  type VisualObservation,
} from './core.js';

type TypeSafeClientLike = Pick<TypeSafeClient, 'systemOne'>;

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

export async function chooseWithTypeSafe(
  client: TypeSafeClientLike,
  candidates: Candidate[],
  snapshot: BrowserSnapshot,
  visual: VisualObservation | undefined,
  history: Record<string, unknown>[]
) {
  const criteria = candidateCriteria(candidates);
  const response = await client.systemOne({
    state: {
      goal: 'Enter the verification token, then submit the form.',
      observation: {
        page: JSON.stringify(snapshot.page ?? null),
        outline: snapshot.outline ?? '',
        visual: JSON.stringify(visualDecisionState(visual)),
      },
      history: JSON.stringify(history),
    },
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
  history: Record<string, unknown>[]
) {
  return chooseWithTypeSafe(new TypeSafeClient(), candidates, snapshot, visual, history);
}

export function chooseMockAdapter(
  candidates: Candidate[],
  _snapshot: BrowserSnapshot,
  _visual: VisualObservation | undefined,
  _history: Record<string, unknown>[]
) {
  return chooseMock(candidates);
}

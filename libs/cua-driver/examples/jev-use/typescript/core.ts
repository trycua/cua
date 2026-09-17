export type Outcome = 'verified' | 'refuted' | 'unknown' | 'abstained' | 'budget_exhausted';

export type Candidate = {
  id: string;
  description: string;
  tool: string | null;
  arguments: Record<string, unknown>;
};

type PageRef = {
  role?: string;
  name?: string | null;
  ref?: string;
  value?: string | null;
};

export type BrowserSnapshot = {
  target_id: string;
  tab_id: string;
  refs?: PageRef[];
  page?: unknown;
  outline?: string;
};

export function buildCandidates(snapshot: BrowserSnapshot, token: string): Candidate[] {
  const common = { target_id: snapshot.target_id, tab_id: snapshot.tab_id };
  const refs = snapshot.refs ?? [];
  const field = refs.find(
    (item) => item.role === 'textbox' && item.name === 'verification value' && item.ref
  );
  const button = refs.find((item) => item.role === 'button' && item.name === 'Submit' && item.ref);
  if (field?.value !== token && field?.ref) {
    return [
      {
        id: 'type-verification-value',
        description: 'Replace the verification field with the required token.',
        tool: 'browser_type',
        arguments: { ...common, ref: field.ref, text: token, replace: true },
      },
      {
        id: 'abstain',
        description:
          'Stop without acting if none of the proposed actions is safe for the observed state.',
        tool: null,
        arguments: {},
      },
    ];
  }
  if (field?.value === token && button?.ref) {
    return [
      {
        id: 'submit-form',
        description: 'Submit the form now that the verification field contains the token.',
        tool: 'browser_click',
        arguments: { ...common, ref: button.ref, input_route: 'dom_event' },
      },
      {
        id: 'abstain',
        description:
          'Stop without acting if none of the proposed actions is safe for the observed state.',
        tool: null,
        arguments: {},
      },
    ];
  }
  return [
    {
      id: 'abstain',
      description: 'Stop without acting because the required controls are unavailable.',
      tool: null,
      arguments: {},
    },
  ];
}

export function chooseMock(candidates: Candidate[]) {
  const ids = new Set(candidates.map((candidate) => candidate.id));
  const selected = ids.has('type-verification-value')
    ? 'type-verification-value'
    : ids.has('submit-form')
      ? 'submit-form'
      : null;
  return {
    choice: selected,
    confidence: selected ? 1 : 0,
    probabilities: Object.fromEntries(
      candidates.map((candidate) => [candidate.id, Number(candidate.id === selected)])
    ),
  };
}

export function validateChoice(choice: string, candidates: Candidate[]): Candidate {
  const candidate = candidates.find((item) => item.id === choice);
  if (!candidate) throw new Error(`provider selected unknown candidate: ${choice}`);
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

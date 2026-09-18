/**
 * Factorized Jev decision recipe for the jev-use example.
 *
 * Ports the factorized question recipe from Kevin's Hermes / oh-my-pi work
 * (Hermes `build_jev_questions` and oh-my-pi `buildFactorizedQuestions`):
 * instead of one opaque judgment, the chooser answers one small `choice`
 * question plus cheap `noul` gate questions. Small questions are easier to
 * grade, mock, and replay than one big one, and the gates give the loop a
 * principled way to stop or re-observe without trusting a single argmax.
 *
 * Adapted to this example's decision shape: candidates are already complete
 * executable Driver actions, so the factorization is `selection` (which
 * candidate) plus two gates -- `goal_achieved` (stop; nothing left to do) and
 * `needs_reobserve` (the state changed; observe again before acting).
 *
 * Any validation problem -- unknown ids, non-finite values, low confidence --
 * returns `null` (fail-open to the caller), never a half-trusted decision.
 */

import { createHash } from 'node:crypto';

import {
  SystemOneHttpClient,
  validateChoiceAnswer,
  validateCriteria,
  validateNoulAnswer,
  type JevBackendName,
  type JevConfig,
  type SystemOneTransport,
} from './jev_backends.js';

export const SELECTION_QUESTION = 'selection';
export const GOAL_ACHIEVED_QUESTION = 'goal_achieved';
export const NEEDS_REOBSERVE_QUESTION = 'needs_reobserve';

/** Gate threshold: a gate fires only when the model is this confident. */
export const GATE_THRESHOLD = 0.7;
/** Minimum choice confidence before the caller should trust the decision. */
export const MIN_CONFIDENCE = 0.4;

export type FactorizedQuestions = Readonly<{
  selection: Readonly<{ type: 'choice'; instructions: string; criteria: Record<string, string> }>;
  goal_achieved: Readonly<{ type: 'noul'; instructions: string }>;
  needs_reobserve: Readonly<{ type: 'noul'; instructions: string }>;
}>;

/** Build the factorized question set for one decision step. */
export function buildFactorizedQuestions(
  candidates: Record<string, string>,
  goal: string
): FactorizedQuestions {
  const criteria = validateCriteria(candidates);
  if (typeof goal !== 'string' || !goal.trim()) {
    throw new Error('goal must be a non-empty string');
  }
  return Object.freeze({
    selection: Object.freeze({
      type: 'choice' as const,
      instructions:
        'Which complete executable action should Cua Driver run next? ' +
        'Select exactly one supplied candidate ID.',
      criteria,
    }),
    goal_achieved: Object.freeze({
      type: 'noul' as const,
      instructions:
        'Is the goal already achieved in the observed state, so that ' +
        'no further action is needed?',
    }),
    needs_reobserve: Object.freeze({
      type: 'noul' as const,
      instructions:
        'Has the observed state changed (stale capture, ambiguous ' +
        'target, missing element) such that fresh observation is ' +
        'required before acting?',
    }),
  });
}

export type FactorizedDecision = Readonly<{
  selectedId: string;
  confidence: number;
  probabilities: Readonly<Record<string, number>>;
  goalAchieved: number;
  needsReobserve: number;
  model?: string;
  backend: JevBackendName;
}>;

export type DecisionPacketBody = Readonly<{
  selected_id: string;
  confidence: number;
  probabilities: Readonly<Record<string, number>>;
  goal_achieved: number;
  needs_reobserve: number;
  model?: string;
  backend: JevBackendName;
  latency_ms: number;
  state_digest: string;
}>;

/**
 * Replayable decision evidence: what was chosen, how sure, on what state.
 *
 * Contains no screenshots, pixels, or secrets -- only ids, scores and a
 * digest of the state the decision was made on, so packets are safe to log
 * and to replay against fixtures.
 */
export class DecisionPacket {
  constructor(
    readonly decision: FactorizedDecision,
    readonly latencyMs: number,
    readonly stateDigest: string
  ) {}

  toDict(): DecisionPacketBody {
    return Object.freeze({
      selected_id: this.decision.selectedId,
      confidence: this.decision.confidence,
      probabilities: { ...this.decision.probabilities },
      goal_achieved: this.decision.goalAchieved,
      needs_reobserve: this.decision.needsReobserve,
      ...(this.decision.model ? { model: this.decision.model } : {}),
      backend: this.decision.backend,
      latency_ms: this.latencyMs,
      state_digest: this.stateDigest,
    });
  }
}

/**
 * Stable digest identifying the decision input (no secret material).
 *
 * Keys are ordered alphabetically so the digest matches the Python
 * implementation's `json.dumps(..., sort_keys=True)` for the same input.
 */
export function stateDigest(
  goal: string,
  candidateIds: readonly string[],
  captureId: string | null | undefined
): string {
  const canonical = JSON.stringify({
    candidates: [...candidateIds].sort(),
    capture_id: captureId ?? null,
    goal,
  });
  return `sha256:${createHash('sha256').update(canonical, 'utf8').digest('hex').slice(0, 16)}`;
}

/**
 * Parse and gate one factorized answer set. `null` means fail-open.
 *
 * Gate order: `goal_achieved` first (stop), then `needs_reobserve`
 * (look again), otherwise the validated selection. Reserved ids must be
 * present for the gates to fire on them.
 */
export function parseFactorizedDecision(
  answers: unknown,
  candidates: Record<string, string>,
  options: { backend?: JevBackendName; model?: string; minConfidence?: number } = {}
): FactorizedDecision | null {
  let criteria: Record<string, string>;
  try {
    criteria = validateCriteria(candidates);
  } catch {
    return null;
  }
  if (!answers || typeof answers !== 'object' || Array.isArray(answers)) return null;
  const record = answers as Record<string, unknown>;
  let selection;
  let goalAchieved: number;
  let needsReobserve: number;
  try {
    selection = validateChoiceAnswer(
      SELECTION_QUESTION,
      record[SELECTION_QUESTION],
      new Set(Object.keys(criteria))
    );
    goalAchieved = validateNoulAnswer(GOAL_ACHIEVED_QUESTION, record[GOAL_ACHIEVED_QUESTION]);
    needsReobserve = validateNoulAnswer(NEEDS_REOBSERVE_QUESTION, record[NEEDS_REOBSERVE_QUESTION]);
  } catch {
    return null;
  }
  const minConfidence = options.minConfidence ?? MIN_CONFIDENCE;
  if (selection.confidence < minConfidence) return null;
  let selectedId = selection.choice;
  if (goalAchieved >= GATE_THRESHOLD && Object.hasOwn(criteria, 'abstain')) {
    selectedId = 'abstain';
  } else if (needsReobserve >= GATE_THRESHOLD && Object.hasOwn(criteria, 'reobserve')) {
    selectedId = 'reobserve';
  }
  const backend = options.backend ?? 'mock';
  return Object.freeze({
    selectedId,
    confidence: selection.confidence,
    probabilities: selection.probabilities,
    goalAchieved,
    needsReobserve,
    ...(options.model ? { model: options.model } : {}),
    backend,
  });
}

/**
 * Run one factorized decision through the configured backend.
 *
 * Returns `null` fail-open when the backend is skipped (missing key,
 * timeout, malformed output) so the caller can fall back or abstain.
 */
export async function chooseFactorized(
  config: JevConfig,
  args: {
    goal: string;
    observation: Record<string, unknown>;
    candidates: Record<string, string>;
    captureId?: string | null;
    transport?: SystemOneTransport;
  }
): Promise<DecisionPacket | null> {
  let criteria: Record<string, string>;
  let questions: FactorizedQuestions;
  try {
    criteria = validateCriteria(args.candidates);
    questions = buildFactorizedQuestions(criteria, args.goal);
  } catch {
    return null;
  }
  if (config.backend === 'mock') {
    const ids = Object.keys(criteria);
    const selected =
      ids.find((id) => id !== 'reobserve' && id !== 'abstain') ??
      (ids.includes('reobserve') ? 'reobserve' : ids[0]);
    const decision: FactorizedDecision = Object.freeze({
      selectedId: selected,
      confidence: 1,
      probabilities: Object.freeze(
        Object.fromEntries(ids.map((id) => [id, id === selected ? 1 : 0]))
      ),
      goalAchieved: 0,
      needsReobserve: 0,
      model: 'mock',
      backend: 'mock' as const,
    });
    return new DecisionPacket(
      decision,
      0,
      stateDigest(args.goal, ids, args.captureId ?? null)
    );
  }
  const started = performance.now();
  const client = new SystemOneHttpClient(config, args.transport);
  try {
    const response = await client.ask({
      state: { goal: args.goal, observation: { ...args.observation } },
      questions: { ...(questions as unknown as Record<string, unknown>) },
    });
    const decision = parseFactorizedDecision(response.answers, criteria, {
      backend: config.backend,
      model: response.model,
    });
    if (!decision) return null;
    return new DecisionPacket(
      decision,
      Math.round((performance.now() - started) * 100) / 100,
      stateDigest(args.goal, Object.keys(criteria), args.captureId ?? null)
    );
  } catch {
    return null;
  }
}

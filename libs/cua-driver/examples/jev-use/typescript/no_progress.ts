/**
 * Bounded no-progress detection for native jev-use runs.
 *
 * Recipe-local only: no provider or Driver contract changes. Successful
 * dispatch is not itself proof of progress. For tasks without an app-owned
 * intermediate score, only repetition of the same delivered candidate is
 * comparable evidence; a different candidate starts a fresh evidence window.
 */
export const NO_PROGRESS_LIMIT = 3;
export type StepKind = 'reobserve' | 'stale' | 'refused' | 'performed';
export type NoProgressStop = Readonly<{ pattern: string; streak: number }>;

export function observedProgressScore(
  taskId: string,
  state: Readonly<Record<string, unknown>>
): number | undefined {
  if (taskId.endsWith('-counter')) {
    const counter = state.counter;
    return typeof counter === 'number' && Number.isInteger(counter) && counter >= 0 ? counter : undefined;
  }
  if (taskId.endsWith('-choose-size')) {
    return Number(state.size === 'large') + Number(state.agreed === true);
  }
  if (taskId === 'canvas-cancel') {
    return Number(state.selected === 'cancel' && state.action_count === 1);
  }
  return undefined;
}

export class NoProgressGuard {
  private bestProgress: number | undefined;
  private pending: { kind: StepKind; candidateId: string } | undefined;
  private streak = 0;
  private recent: string[] = [];

  constructor(
    readonly taskId: string,
    readonly limit: number = NO_PROGRESS_LIMIT
  ) {
    if (!Number.isInteger(limit) || limit < 1) throw new Error('no-progress limit must be positive');
  }

  note(kind: StepKind, candidateId: string): void {
    this.pending = { kind, candidateId: candidateId.replace(/:foreground$/, '') };
  }

  beforeStep(oracleState: Readonly<Record<string, unknown>>): NoProgressStop | undefined {
    const score = observedProgressScore(this.taskId, oracleState);
    const pending = this.pending;
    this.pending = undefined;

    if (score !== undefined && (this.bestProgress === undefined || score > this.bestProgress)) {
      this.bestProgress = score;
      this.resetEvidence();
      return undefined;
    }

    if (!pending) return undefined;
    const token =
      pending.kind === 'performed' ? `performed:${pending.candidateId}` : pending.kind;

    if (pending.kind === 'performed' && score === undefined) {
      if (this.recent.length && this.recent.some((item) => item !== token)) {
        this.resetEvidence();
      }
    }

    return this.record(token);
  }

  private record(token: string): NoProgressStop | undefined {
    this.streak += 1;
    this.recent.push(token);
    this.recent = this.recent.slice(-this.limit);
    if (this.streak < this.limit) return undefined;
    return { pattern: this.pattern(), streak: this.streak };
  }

  private resetEvidence(): void {
    this.streak = 0;
    this.recent = [];
  }

  private pattern(): string {
    if (this.recent.length && this.recent.every((item) => item === 'reobserve')) return 'reobserve';
    if (this.recent.length && this.recent.every((item) => item.startsWith('performed:'))) {
      if (new Set(this.recent).size === 1) return 'same_candidate';
    }
    if (this.recent.some((item) => item === 'stale' || item === 'refused')) return 'recovery';
    return 'unchanged_progress';
  }
}

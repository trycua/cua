/**
 * Task specs: what one jev-use run is trying to achieve and how it is checked.
 *
 * A task spec owns everything that is specific to one application task: the
 * goal shown to the model, its parameters (secrets are redacted from all model
 * input), the step budget, the Driver action kinds it may use, the compact
 * state summary the model sees, the candidate IDs and descriptions it offers,
 * and the success oracle that verifies the outcome independently of the
 * runner's own events.
 *
 * Candidate sources (sources.ts) supply the controls and build the executable
 * candidates. The runner stays task-agnostic: it observes, asks the task for
 * candidates and a state summary, lets the model choose one supplied ID, acts,
 * and asks the task's oracle whether the task is done.
 *
 * FixtureFormTask is the one built-in task: type a secret verification token
 * into the loopback fixture's form and submit it, verified through the
 * fixture's /state endpoint.
 */
import type { BrowserSnapshot, VisualDelivery, VisualObservation, VisualRegion } from './core.js';
import {
  BrowserSemanticSource,
  immutableCandidate,
  VisualRegionSource,
  type Candidate,
  type NativeAccessibilitySource,
} from './sources.js';

export type Outcome = 'verified' | 'refuted' | 'unknown' | 'abstained' | 'budget_exhausted';

export const REDACTED_TOKEN = '[verification token]';

/** Replace every occurrence of the token in strings nested in value. */
export function redactToken(value: unknown, token: string, placeholder = REDACTED_TOKEN): unknown {
  if (!token) return value;
  if (typeof value === 'string') return value.split(token).join(placeholder);
  if (Array.isArray(value)) return value.map((item) => redactToken(item, token, placeholder));
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, redactToken(item, token, placeholder)])
    );
  }
  return value;
}

/**
 * A named task input. A secret value is replaced by redaction everywhere in
 * model input; it may still appear in executable arguments.
 */
export type TaskParameter = Readonly<{
  name: string;
  value: string;
  secret: boolean;
  redaction: string;
}>;

/**
 * The candidate sources available for one decision step. visual is present
 * only when this step parsed visual regions. visualPath reports whether a
 * capture-bound visual path exists at all, so a task can say a control is still
 * pending a visual check.
 */
export type TaskSources = Readonly<{
  /** Present for browser tasks. */
  page?: BrowserSemanticSource;
  visual?: VisualRegionSource;
  visualPath: boolean;
  /** Present for native tasks (RFC #4268). */
  ax?: NativeAccessibilitySource;
  /** Native candidate IDs whose background delivery Driver refused earlier. */
  foregroundIds?: ReadonlySet<string>;
}>;

export function requirePage(sources: TaskSources): BrowserSemanticSource {
  if (!sources.page) throw new Error('this task needs a browser page source');
  return sources.page;
}

/**
 * One runner history item. `performed` marks a dispatched, successful native
 * action; it stays in the runner and is never sent to a provider.
 */
export type HistoryEntry = Readonly<{ step: number; selected_id: string; outcome: string; performed?: true }>;

/** The interface the runner uses for every task. */
export interface Task {
  readonly id: string;
  readonly goal: string;
  readonly parameters: readonly TaskParameter[];
  readonly maxSteps: number;
  readonly allowedActionKinds: ReadonlySet<string>;
  /** Candidate IDs after which the runner polls the oracle for completion. */
  readonly completionCandidateIds: ReadonlySet<string>;
  redact(value: unknown): unknown;
  candidates(sources: TaskSources): Candidate[];
  stateSummary(sources: TaskSources): Readonly<Record<string, string>>;
  historyEntry(step: number, candidateId: string, refusal?: string): HistoryEntry;
  reset(): Promise<void>;
  readOracle(): Promise<Readonly<Record<string, unknown>>>;
  classify(oracle: Readonly<Record<string, unknown>>, steps: number): Outcome;
}

// The built-in browser fixture task.

export const FIXTURE_TASK_ID = 'browser-fixture-form';
export const FIXTURE_GOAL =
  'Enter the required verification token into the verification field, then submit the form.';
export const DEFAULT_FIXTURE_URL = 'http://127.0.0.1:8765/';
export const DEFAULT_MAX_STEPS = 4;
export const FIELD_NAME = 'verification value';
export const SUBMIT_NAME = 'Submit';
export const SUBMIT_IDS: ReadonlySet<string> = new Set(['submit-form', 'submit-form-foreground']);
export const FIXTURE_ACTION_KINDS: ReadonlySet<string> = new Set([
  'browser_type',
  'browser_click',
  'click',
]);

export type SubmitButtonState =
  | 'available'
  | 'visual_only'
  | 'visual_check_pending'
  | 'not_found_visually'
  | 'not_in_page_structure';

export type FormState = Readonly<{
  verification_field: 'not_found' | 'empty' | 'contains_required_token' | 'contains_other_value';
  submit_button: SubmitButtonState;
}>;

export async function fixtureState(fixtureUrl: string): Promise<{ submitted: string | null }> {
  const response = await fetch(new URL('state', fixtureUrl));
  if (!response.ok) throw new Error(`fixture state failed: HTTP ${response.status}`);
  return (await response.json()) as { submitted: string | null };
}

export async function resetFixture(fixtureUrl: string): Promise<void> {
  const response = await fetch(new URL('reset', fixtureUrl), { method: 'POST' });
  if (response.status !== 204) throw new Error(`fixture reset failed: HTTP ${response.status}`);
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

export const HISTORY_OUTCOMES: Readonly<Record<string, string>> = {
  'type-verification-value': 'typed the required token into the verification field',
  'submit-form': 'clicked Submit; the submission was not yet confirmed',
  'submit-form-foreground':
    'clicked Submit in the foreground; the submission was not yet confirmed',
  reobserve: 'took no action and requested a fresh observation',
};

/**
 * Build the compact decision-history item shown to the model. It records what
 * each step did, not timings or model probabilities, so earlier choices do not
 * become a signal to repeat themselves.
 */
export function historyEntry(step: number, candidateId: string, refusal?: string): HistoryEntry {
  const outcome = refusal
    ? `Driver refused background delivery (${refusal}); no click happened and a ` +
      'foreground Submit candidate is offered next'
    : (HISTORY_OUTCOMES[candidateId] ?? 'completed');
  return { step, selected_id: candidateId, outcome };
}

function reservedCandidates(): Candidate[] {
  return [
    immutableCandidate({
      id: 'reobserve',
      description:
        'Take no action and obtain a fresh Driver observation, because the current ' +
        'observation is stale or contradicts the reported form state. A Submit control ' +
        'that is visual-only, or whose visual check is still pending, is not a reason to ' +
        'reobserve: the runner parses visual regions for Submit once no page-structure ' +
        'action remains.',
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

/**
 * Type the secret token into the fixture form and submit it. The oracle is the
 * fixture's /state endpoint: the task is verified only when the fixture
 * recorded exactly this token as submitted.
 */
export class FixtureFormTask implements Task {
  readonly id = FIXTURE_TASK_ID;
  readonly goal = FIXTURE_GOAL;
  readonly allowedActionKinds = FIXTURE_ACTION_KINDS;
  readonly completionCandidateIds = SUBMIT_IDS;

  constructor(
    readonly token: string,
    readonly fixtureUrl: string = DEFAULT_FIXTURE_URL,
    readonly maxSteps: number = DEFAULT_MAX_STEPS
  ) {}

  get parameters(): readonly TaskParameter[] {
    return [
      { name: 'verification_token', value: this.token, secret: true, redaction: REDACTED_TOKEN },
    ];
  }

  redact(value: unknown): unknown {
    let result = value;
    for (const parameter of this.parameters) {
      if (parameter.secret) result = redactToken(result, parameter.value, parameter.redaction);
    }
    return result;
  }

  /**
   * Build the closed candidate set for one decision. Page-structure refs always
   * win. The capture-bound visual Submit is offered only when no Submit ref
   * exists. A visual source with foreground delivery yields a distinct
   * submit-form-foreground candidate after Driver refused background delivery;
   * the chooser must pick it explicitly.
   */
  candidates(sources: TaskSources): Candidate[] {
    const { visual } = sources;
    const page = requirePage(sources);
    const token = this.token;
    const field = page.find('textbox', FIELD_NAME);
    const button = page.find('button', SUBMIT_NAME);
    const candidates: Candidate[] = [];
    if (field && field.value !== token) {
      candidates.push(
        page.typeText(
          field,
          token,
          'type-verification-value',
          'Type the required verification token into the verification field, ' +
            'replacing its current contents.'
        )
      );
    } else if (field && field.value === token && button) {
      candidates.push(
        page.click(
          button,
          'submit-form',
          "Click the form's Submit button. The observed form state reports that the " +
            'verification field already contains the required token, so the form is ' +
            'ready to submit.'
        )
      );
    } else if (field && field.value === token && visual) {
      const region = visual.find('button', SUBMIT_NAME);
      if (region) {
        const foreground = visual.delivery === 'foreground';
        const candidate = visual.click(
          region,
          foreground ? 'submit-form-foreground' : 'submit-form',
          foreground
            ? 'Submit the form by clicking the unique validated visual Submit region with ' +
                'foreground delivery, which activates the browser window, because Driver ' +
                'refused background delivery for the previous visual click.'
            : 'Submit the form by clicking the unique validated visual Submit region. ' +
                'The observed form state reports that the verification field already ' +
                'contains the required token.'
        );
        if (candidate) candidates.push(candidate);
      }
    }
    for (const candidate of candidates) {
      if (candidate.tool === null || !this.allowedActionKinds.has(candidate.tool)) {
        throw new Error(`task ${this.id} does not allow action kind ${candidate.tool}`);
      }
    }
    return [...candidates, ...reservedCandidates()];
  }

  /**
   * Summarize the form for the decision model without revealing the token. The
   * raw field value never leaves the runner.
   *
   * submit_button is 'available' for a clickable page-structure ref. When the
   * page structure has none and the capture-bound visual path is enabled
   * (visualPath), it is 'visual_only' if this observation holds a unique
   * validated visual Submit region, 'visual_check_pending' if no visual regions
   * were parsed for this step yet (the runner parses them once no page-structure
   * action remains), and 'not_found_visually' otherwise. Without a visual path
   * it is 'not_in_page_structure'.
   */
  stateSummary(sources: TaskSources): FormState {
    const page = requirePage(sources);
    const field = page.find('textbox', FIELD_NAME);
    const button = page.find('button', SUBMIT_NAME);
    const verificationField = !field
      ? 'not_found'
      : !field.value
        ? 'empty'
        : field.value === this.token
          ? 'contains_required_token'
          : 'contains_other_value';
    const submitButton: SubmitButtonState = button
      ? 'available'
      : !sources.visualPath
        ? 'not_in_page_structure'
        : !sources.visual
          ? 'visual_check_pending'
          : sources.visual.find('button', SUBMIT_NAME)
            ? 'visual_only'
            : 'not_found_visually';
    return { verification_field: verificationField, submit_button: submitButton };
  }

  historyEntry(step: number, candidateId: string, refusal?: string): HistoryEntry {
    return historyEntry(step, candidateId, refusal);
  }

  reset(): Promise<void> {
    return resetFixture(this.fixtureUrl);
  }

  readOracle(): Promise<{ submitted: string | null }> {
    return fixtureState(this.fixtureUrl);
  }

  classify(oracle: Readonly<Record<string, unknown>>, steps: number): Outcome {
    return classify(oracle.submitted as string | null, this.token, steps, this.maxSteps);
  }
}

/** Build one step's sources from a browser snapshot and optional visual parse. */
export function fixtureSources(
  snapshot: BrowserSnapshot,
  visual?: VisualObservation,
  captureBoundClick = false,
  visualDelivery: VisualDelivery = 'background',
  visualPath = false
): TaskSources {
  return {
    page: new BrowserSemanticSource(snapshot),
    visual: visual ? new VisualRegionSource(visual, visualDelivery, captureBoundClick) : undefined,
    visualPath,
  };
}

// Compatibility entry points. They keep the pre-task-spec signatures and
// delegate to the built-in fixture task, so existing callers are unchanged.

/** Return the unique validated visual Submit region, or undefined. */
export function visualSubmitRegion(visual?: VisualObservation): VisualRegion | undefined {
  if (!visual) return undefined;
  return new VisualRegionSource(visual).find('button', SUBMIT_NAME)?.handle;
}

/** Summarize the fixture form; see FixtureFormTask.stateSummary. */
export function formState(
  snapshot: BrowserSnapshot,
  token: string,
  visual?: VisualObservation,
  visualPath = false
): FormState {
  return new FixtureFormTask(token).stateSummary(
    fixtureSources(snapshot, visual, false, 'background', visualPath)
  );
}

/** Build the fixture candidate set; see FixtureFormTask.candidates. */
export function buildCandidates(
  snapshot: BrowserSnapshot,
  token: string,
  visual?: VisualObservation,
  captureBoundClick = false,
  visualDelivery: VisualDelivery = 'background'
): Candidate[] {
  return new FixtureFormTask(token).candidates(
    fixtureSources(snapshot, visual, captureBoundClick, visualDelivery)
  );
}

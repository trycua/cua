/**
 * Native task specs for jev-use (RFC #4268, Phase 1). Mirrors
 * python/native_tasks.py: a NativeTask declares goal, parameters (the only
 * source of text), window scope, allowed action kinds, opt-in risks,
 * foreground permission, step budget, and an app-owned oracle. The built-in
 * tasks drive the AppKit, WPF, WinUI3, and GTK3 harnesses in task mode
 * (CUA_APPKIT_TASK_STATE, CUA_WPF_TASK_STATE, CUA_WINUI3_TASK_STATE,
 * CUA_GTK3_TASK_STATE), whose JSON state file is the oracle.
 */
import { readFile } from 'node:fs/promises';

import {
  MAX_ELEMENTS,
  MAX_PROGRESS,
  MAX_PROGRESS_COUNT,
  REQUEST_SCHEMA_V2,
  type ProgressItem,
} from './choose_action.js';
import {
  PARAMETER_NAME_PATTERN,
  elementState,
  fieldState,
  hasApplicationElements,
  slug,
  type NativeControl,
} from './native.js';
import { ACTION_KINDS, type ActionKind } from './native_roles.js';
import {
  immutableCandidate,
  VisualRegionSource,
  type Candidate,
  type NativeAccessibilitySource,
  type TextMethod,
} from './sources.js';
import { redactToken, type HistoryEntry, type Outcome, type Task, type TaskParameter, type TaskSources } from './tasks.js';

export const MAX_EXECUTABLE_CANDIDATES = 24;
const MAX_HISTORY = 16;
const SOURCE_ORDER = ['page', 'ax', 'visual'] as const;
export type Check = 'verified' | 'refuted' | 'pending';

export const NATIVE_RESERVED: readonly Candidate[] = [
  immutableCandidate({
    id: 'reobserve',
    description:
      'Take no action and obtain a fresh observation of the window, because the current ' +
      "observation looks stale, incomplete, or contradicts the goal's progress.",
    tool: null,
    arguments: {},
  }),
  immutableCandidate({
    id: 'abstain',
    description:
      'Stop without acting if none of the proposed actions is safe or moves toward the goal.',
    tool: null,
    arguments: {},
  }),
];

// Words too common to make a control relevant to a goal (#4312).
const RELEVANCE_STOPWORDS = new Set([
  'the', 'and', 'then', 'once', 'per', 'step', 'stop', 'into', 'with', 'for', 'from',
  'this', 'that', 'its', 'each', 'exactly', 'starts', 'set', 'option', 'field', 'button',
]);

function relevanceWords(text: string): Set<string> {
  return new Set(
    (text.toLowerCase().match(/[a-z0-9]+/g) ?? []).filter(
      (word) => word.length >= 3 && !RELEVANCE_STOPWORDS.has(word)
    )
  );
}

export type CapOrder = 'relevance' | 'depth_first';

const VERBS: Readonly<Record<string, string>> = {
  press: 'pressed',
  toggle: 'toggled',
  select: 'selected',
  open_menu: 'opened',
  visual_click: 'clicked the visual region',
};

export type WindowScope = Readonly<{
  windowTitle: string;
  bundleId?: string;
  processName?: string;
  query?: string;
  maxElements?: number;
  maxDepth?: number;
}>;

export function windowStateArguments(scope: WindowScope): Record<string, unknown> {
  return {
    ...(scope.query !== undefined ? { query: scope.query } : {}),
    ...(scope.maxElements !== undefined ? { max_elements: scope.maxElements } : {}),
    ...(scope.maxDepth !== undefined ? { max_depth: scope.maxDepth } : {}),
  };
}

export class OracleError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'OracleError';
  }
}

/** An app_check oracle over an app-owned JSON state file bound to one pid. */
export class AppStateOracle {
  constructor(
    readonly path: string,
    readonly schema: string,
    readonly expectedPid?: number
  ) {}

  async read(): Promise<Record<string, unknown>> {
    let state: unknown;
    try {
      state = JSON.parse(await readFile(this.path, 'utf8'));
    } catch (error) {
      throw new OracleError(`app state is unreadable: ${(error as Error).name}`);
    }
    if (!state || typeof state !== 'object' || (state as Record<string, unknown>).schema !== this.schema) {
      throw new OracleError('app state has an unexpected schema');
    }
    if (this.expectedPid !== undefined && (state as Record<string, unknown>).pid !== this.expectedPid) {
      throw new OracleError('app state belongs to a different process');
    }
    return state as Record<string, unknown>;
  }
}

export type ComposeStats = Readonly<{
  sources: Readonly<Record<string, number>>;
  duplicates: number;
  risk_excluded: Readonly<Record<string, number>>;
  dropped: number;
}>;

/**
 * Merge source outputs in the order page, ax, visual: first source wins on a
 * duplicate ID, unallowed risk categories are removed, at most `cap`
 * executable candidates remain (the drop count is reported), and the reserved
 * candidates are appended. Without `relevance`, the first `cap` in
 * depth-first order are kept. With `relevance` (lower is more relevant), a set
 * over the cap keeps the `cap` lowest `(relevance, position)` candidates and
 * still presents them in depth-first order (#4312). A set within the cap is
 * unchanged.
 */
export function compose(
  groups: Partial<Record<(typeof SOURCE_ORDER)[number], readonly Candidate[]>>,
  allowedRisks: ReadonlySet<string>,
  reserved: readonly Candidate[] = NATIVE_RESERVED,
  cap = MAX_EXECUTABLE_CANDIDATES,
  relevance?: (candidate: Candidate) => number
): { candidates: Candidate[]; stats: ComposeStats } {
  const seen = new Set(reserved.map((candidate) => candidate.id));
  const merged: Candidate[] = [];
  let duplicates = 0;
  const riskExcluded: Record<string, number> = {};
  for (const source of SOURCE_ORDER) {
    for (const candidate of groups[source] ?? []) {
      if (seen.has(candidate.id)) {
        duplicates += 1;
        continue;
      }
      seen.add(candidate.id);
      const blocked = [...(candidate.risk ?? [])].filter((risk) => !allowedRisks.has(risk)).sort();
      if (blocked.length) {
        for (const category of blocked) riskExcluded[category] = (riskExcluded[category] ?? 0) + 1;
        continue;
      }
      merged.push(candidate);
    }
  }
  let kept: Candidate[];
  if (!relevance || merged.length <= cap) {
    kept = merged.slice(0, cap);
  } else {
    const ranked = merged
      .map((candidate, index) => ({ index, rank: relevance(candidate) }))
      .sort((a, b) => a.rank - b.rank || a.index - b.index)
      .slice(0, cap)
      .map((entry) => entry.index)
      .sort((a, b) => a - b);
    kept = ranked.map((index) => merged[index]);
  }
  const counts: Record<string, number> = { page: 0, ax: 0, visual: 0 };
  for (const candidate of kept) {
    if (candidate.source && candidate.source in counts) counts[candidate.source] += 1;
  }
  return {
    candidates: [...kept, ...reserved],
    stats: { sources: counts, duplicates, risk_excluded: riskExcluded, dropped: merged.length - kept.length },
  };
}

export type CompactElement = { role_class: string; label: string; state: string };

/**
 * One step a task requires, counted from the runner's own performed actions.
 * `candidateId` names the candidate that performs the step; its `:foreground`
 * variant counts too. The description is task-authored and value-free. A step
 * with `afterPrevious` (the default) waits for every earlier step to be done.
 */
export type TaskStep = Readonly<{
  description: string;
  candidateId: string;
  times?: number;
  afterPrevious?: boolean;
}>;

/**
 * Count the actions this run dispatched successfully, by candidate ID. Only
 * entries marked `performed` count; nothing is read from the application.
 */
export function performedCounts(history: readonly HistoryEntry[]): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const item of history) {
    if (item.performed !== true) continue;
    const base = item.selected_id.replace(/:foreground$/, '');
    counts[base] = (counts[base] ?? 0) + 1;
  }
  return counts;
}

export type NativeStep = Readonly<{
  candidates: Candidate[];
  stats: ComposeStats;
  elements: CompactElement[];
  outcomes: Readonly<Record<string, string>>;
}>;

function quoted(label: string, limit = 60): string {
  return JSON.stringify(label.length <= limit ? label : `${label.slice(0, limit - 1)}…`);
}

export type NativeTaskSpec = Readonly<{
  id: string;
  goal: string;
  scope: WindowScope;
  allowedActions: ReadonlySet<ActionKind>;
  oracle: AppStateOracle;
  check: (state: Readonly<Record<string, unknown>>) => Check;
  parameters?: readonly TaskParameter[];
  allowedRisks?: ReadonlySet<string>;
  allowForeground?: boolean;
  maxSteps?: number;
  textMethod?: TextMethod;
  visualTargets?: readonly string[];
  visualMinConfidence?: number;
  mockPreferences?: readonly string[];
  steps?: readonly TaskStep[];
  capOrder?: CapOrder;
}>;

export class NativeTask implements Task {
  readonly id: string;
  readonly goal: string;
  readonly scope: WindowScope;
  readonly allowedActions: ReadonlySet<ActionKind>;
  readonly oracle: AppStateOracle;
  readonly check: NativeTaskSpec['check'];
  readonly parameters: readonly TaskParameter[];
  readonly allowedRisks: ReadonlySet<string>;
  readonly allowForeground: boolean;
  readonly maxSteps: number;
  readonly textMethod: TextMethod;
  readonly visualTargets: readonly string[];
  /** OCR confidence a visual target must reach; exact-text uniqueness still applies. */
  readonly visualMinConfidence: number;
  readonly mockPreferences: readonly string[];
  /**
   * Ordered steps the task requires (#4313). The request reports how often
   * this run has performed each, and a step's candidate names any earlier
   * step that is not done yet. Empty means the request carries no progress.
   */
  readonly steps: readonly TaskStep[];
  /**
   * How a set over the cap is cut (#4312): 'relevance' keeps the declared
   * steps' candidates first; 'depth_first' keeps the first `cap` in element
   * order, as before. Both present the kept candidates in element order.
   */
  readonly capOrder: CapOrder;
  /** The oracle is polled after every action, so no candidate is special. */
  readonly completionCandidateIds: ReadonlySet<string> = new Set();

  constructor(spec: NativeTaskSpec) {
    this.id = spec.id;
    this.goal = spec.goal;
    this.scope = spec.scope;
    this.allowedActions = spec.allowedActions;
    this.oracle = spec.oracle;
    this.check = spec.check;
    this.parameters = spec.parameters ?? [];
    this.allowedRisks = spec.allowedRisks ?? new Set();
    this.allowForeground = spec.allowForeground ?? false;
    this.maxSteps = spec.maxSteps ?? 6;
    this.textMethod = spec.textMethod ?? 'set_value';
    this.visualTargets = spec.visualTargets ?? [];
    this.visualMinConfidence = spec.visualMinConfidence ?? 0.8;
    this.mockPreferences = spec.mockPreferences ?? [];
    this.steps = spec.steps ?? [];
    this.capOrder = spec.capOrder ?? 'relevance';
    if (this.capOrder !== 'relevance' && this.capOrder !== 'depth_first') {
      throw new Error('capOrder must be relevance or depth_first');
    }
    const unknown = [...this.allowedActions].filter((action) => !ACTION_KINDS.has(action));
    if (unknown.length) throw new Error(`unknown action kinds: ${unknown.sort().join(', ')}`);
    for (const parameter of this.parameters) {
      if (!PARAMETER_NAME_PATTERN.test(parameter.name)) {
        throw new Error('parameter names must match [a-z][a-z0-9_]{0,7}');
      }
    }
    if (!Number.isInteger(this.maxSteps) || this.maxSteps < 1) throw new Error('maxSteps must be positive');
    if (this.steps.length > MAX_PROGRESS) throw new Error(`a task declares at most ${MAX_PROGRESS} steps`);
    for (const taskStep of this.steps) {
      const times = taskStep.times ?? 1;
      if (!Number.isInteger(times) || times < 1 || times > MAX_PROGRESS_COUNT) {
        throw new Error(`step times must be from 1 to ${MAX_PROGRESS_COUNT}`);
      }
    }
  }

  get allowedActionKinds(): ReadonlySet<string> {
    const tools = new Set<string>();
    if ([...this.allowedActions].some((action) => action !== 'set_text')) tools.add('click');
    if (this.allowedActions.has('set_text')) tools.add(this.textMethod);
    return tools;
  }

  redact(value: unknown): unknown {
    let result = value;
    for (const parameter of this.parameters) {
      if (parameter.secret) result = redactToken(result, parameter.value, parameter.redaction);
    }
    return result;
  }

  redactText = (value: string): string => this.redact(value) as string;

  private nativeCandidates(ax: NativeAccessibilitySource, foregroundIds: ReadonlySet<string>) {
    const candidates: Candidate[] = [];
    const outcomes: Record<string, string> = {};
    const labels: Record<string, string> = {};
    for (const native of ax.controls) {
      if (!this.allowedActions.has(native.action)) continue;
      const control = ax.control(native);
      const label = quoted(native.label);
      if (native.action === 'set_text') {
        for (const parameter of this.parameters) {
          if (native.value === parameter.value) continue;
          const id = `${native.id}:set:${parameter.name}`;
          candidates.push(
            ax.typeText(
              control,
              parameter.value,
              id,
              `Set the text field ${label} to the task parameter ${JSON.stringify(parameter.name)}, ` +
                `replacing its contents. The field currently reports ${fieldState(native.value, parameter.value)}.`
            )
          );
          outcomes[id] = `set ${label} to parameter ${parameter.name}`;
          labels[id] = native.label;
        }
        continue;
      }
      if (native.action === 'select' && native.selected) continue;
      let description = NativeTask.describe(native, label);
      let id = native.id;
      let delivery: 'background' | 'foreground' = 'background';
      if (foregroundIds.has(native.id)) {
        if (!this.allowForeground) continue;
        id = `${native.id}:foreground`;
        delivery = 'foreground';
        description +=
          ' Use foreground delivery, which activates the window, because Driver refused ' +
          'background delivery for this control.';
      }
      candidates.push(ax.click(control, id, description, delivery));
      outcomes[id] = `${VERBS[native.action]} ${label}`;
      labels[id] = native.label;
    }
    return { candidates, outcomes, labels };
  }

  static describe(native: NativeControl, label: string): string {
    if (native.action === 'toggle') {
      const now = native.selected ? 'checked' : 'unchecked';
      const after = native.selected ? 'unchecked' : 'checked';
      const noun = native.roleClass === 'toggle' ? 'switch' : 'checkbox';
      return `Toggle the ${noun} ${label}. It is currently ${now}; afterward it will be ${after}.`;
    }
    if (native.action === 'select') return `Select the radio option ${label}. It is currently not selected.`;
    if (native.action === 'open_menu') {
      return `Open the pop-up menu ${label}. Its options become candidates on the next observation.`;
    }
    const noun = native.roleClass === 'menu_item' ? 'menu item' : native.roleClass === 'link' ? 'link' : 'button';
    return `Press the ${noun} labeled ${label}.`;
  }

  private visualCandidates(sources: TaskSources) {
    const candidates: Candidate[] = [];
    const outcomes: Record<string, string> = {};
    const visual = sources.visual;
    if (!visual || !this.allowedActions.has('visual_click')) return { candidates, outcomes };
    for (const target of this.visualTargets) {
      const control = visual.find('button', target);
      if (!control) continue;
      let id = `visual:${slug(target)}`;
      let description = `Click the unique validated visual region reading ${quoted(target)}; no accessibility element covers it.`;
      let source = visual;
      if (sources.foregroundIds?.has(id)) {
        if (!this.allowForeground) continue;
        id = `${id}:foreground`;
        description +=
          ' Use foreground delivery, which activates the window, because Driver refused background delivery for this region.';
        source = new VisualRegionSource(visual.observation, 'foreground', visual.captureBound, visual.minConfidence);
      }
      const candidate = source.click(control, id, description);
      if (candidate) {
        candidates.push(candidate);
        outcomes[id] = `clicked the visual region ${quoted(target)}`;
      }
    }
    return { candidates, outcomes };
  }

  /**
   * Rank candidates for the cap only (#4312); lower is more relevant. Tier 0
   * performs a declared step or clicks a declared visual target; tier 1 has a
   * label sharing a word with the goal; tier 2 is everything else. Uses only
   * task-authored text and labels, never values.
   */
  relevance(labels: Readonly<Record<string, string>>): (candidate: Candidate) => number {
    const declared = new Set([
      ...this.steps.map((taskStep) => taskStep.candidateId),
      ...this.visualTargets.map((target) => `visual:${slug(target)}`),
    ]);
    const goalWords = relevanceWords(this.goal);
    return (candidate) => {
      if (declared.has(candidate.id.replace(/:foreground$/, ''))) return 0;
      for (const word of relevanceWords(labels[candidate.id] ?? '')) if (goalWords.has(word)) return 1;
      return 2;
    };
  }

  /**
   * The candidate IDs that correctly advance the task now, for measurement: a
   * declared step performed fewer times than required whose earlier steps
   * (for a step that waits) are done. Counted from this run's own actions.
   */
  expectedNext(history: readonly HistoryEntry[]): string[] {
    const counts = performedCounts(history);
    const due: string[] = [];
    this.steps.forEach((taskStep, index) => {
      if ((counts[taskStep.candidateId] ?? 0) >= (taskStep.times ?? 1)) return;
      const earlier = taskStep.afterPrevious === false ? [] : this.steps.slice(0, index);
      if (earlier.every((step) => (counts[step.candidateId] ?? 0) >= (step.times ?? 1))) {
        due.push(taskStep.candidateId);
      }
    });
    return due;
  }

  plan(sources: TaskSources): NativeStep {
    const outcomes: Record<string, string> = {};
    let labels: Record<string, string> = {};
    let axCandidates: Candidate[] = [];
    let elements: CompactElement[] = [];
    if (sources.ax) {
      const native = this.nativeCandidates(sources.ax, sources.foregroundIds ?? new Set());
      axCandidates = native.candidates;
      labels = native.labels;
      Object.assign(outcomes, native.outcomes);
      elements = sources.ax.controls.slice(0, MAX_ELEMENTS).map((control) => ({
        role_class: control.roleClass,
        label: control.label,
        state: elementState(control),
      }));
    }
    const visual = this.visualCandidates(sources);
    Object.assign(outcomes, visual.outcomes);
    const { candidates, stats } = compose(
      { ax: axCandidates, visual: visual.candidates },
      this.allowedRisks,
      NATIVE_RESERVED,
      MAX_EXECUTABLE_CANDIDATES,
      this.capOrder === 'relevance' ? this.relevance(labels) : undefined
    );
    for (const candidate of candidates) {
      if (candidate.tool !== null && !this.allowedActionKinds.has(candidate.tool)) {
        throw new Error(`task ${this.id} does not allow action kind ${candidate.tool}`);
      }
    }
    return { candidates, stats, elements, outcomes };
  }

  candidates(sources: TaskSources): Candidate[] {
    return this.plan(sources).candidates;
  }

  stateSummary(sources: TaskSources): Readonly<Record<string, string>> {
    return Object.fromEntries((sources.ax?.controls ?? []).map((control) => [control.id, elementState(control)]));
  }

  historyEntry(
    step: number,
    candidateId: string,
    refusal?: string,
    options: { outcome?: string; stale?: boolean } = {}
  ): HistoryEntry {
    const text = options.stale
      ? 'the observation was stale; nothing happened and the window is observed again'
      : refusal !== undefined
        ? `Driver refused background delivery (${refusal}); nothing happened`
        : candidateId === 'reobserve'
          ? 'took no action and requested a fresh observation'
          : (options.outcome ?? 'completed');
    const performed =
      !options.stale && refusal === undefined && candidateId !== 'reobserve' && candidateId !== 'abstain';
    return {
      step,
      selected_id: candidateId,
      outcome: (this.redact(text) as string).slice(0, 128),
      ...(performed ? { performed: true as const } : {}),
    };
  }

  /** Each declared step and how often this run has performed it. */
  progress(history: readonly HistoryEntry[]): ProgressItem[] {
    const counts = performedCounts(history);
    return this.steps.map((taskStep) => ({
      step: this.redact(taskStep.description) as string,
      done: Math.min(counts[taskStep.candidateId] ?? 0, MAX_PROGRESS_COUNT),
      required: taskStep.times ?? 1,
    }));
  }

  /**
   * A sentence stating a candidate's place in the task's declared steps: a
   * done step says so; a step with an earlier step not yet done names it; a
   * due step says how many more times the task requires it.
   */
  stepNote(candidateId: string, history: readonly HistoryEntry[]): string {
    const base = candidateId.replace(/:foreground$/, '');
    const counts = performedCounts(history);
    const index = this.steps.findIndex((taskStep) => taskStep.candidateId === base);
    if (index < 0) return '';
    const taskStep = this.steps[index];
    const times = taskStep.times ?? 1;
    if ((counts[base] ?? 0) >= times) {
      return ` This run already did this the ${times} time(s) the task requires.`;
    }
    const pending = (taskStep.afterPrevious === false ? [] : this.steps.slice(0, index))
      .filter((earlier) => (counts[earlier.candidateId] ?? 0) < (earlier.times ?? 1))
      .map((earlier) => earlier.description);
    if (pending.length) return ` The task requires this only after: ${pending.join('; ')} (not done yet).`;
    return ` The task still requires this ${times - (counts[base] ?? 0)} more time(s).`;
  }

  async reset(): Promise<void> {
    // The harness starts fresh for every run.
  }

  readOracle(): Promise<Record<string, unknown>> {
    return this.oracle.read();
  }

  classify(oracle: Readonly<Record<string, unknown>>, steps: number): Outcome {
    const result = this.check(oracle);
    if (result === 'verified' || result === 'refuted') return result;
    return steps >= this.maxSteps ? 'budget_exhausted' : 'unknown';
  }
}

/** Why this step may parse visual regions, or undefined (see python/native_tasks.py). */
export function visualFallbackReason(
  sources: TaskSources,
  task: NativeTask,
  nativeCount: number
): string | undefined {
  const ax = sources.ax;
  if (!ax || !task.allowedActions.has('visual_click') || !task.visualTargets.length) return undefined;
  const observation = ax.observation;
  if (!observation.captureId) return undefined;
  if (observation.degradedReason?.startsWith('ax_tree_empty')) return 'tree_empty';
  if (observation.truncated) return undefined;
  // A partial tree qualifies only when it holds nothing but window roots and
  // window chrome, as for a custom-painted surface.
  if (!observation.complete) {
    return hasApplicationElements(observation, ax.platform) ? undefined : 'no_application_elements';
  }
  if (nativeCount === 0) return 'no_native_candidates';
  const labels = new Set(ax.controls.map((control) => control.label.toLowerCase()));
  if (task.visualTargets.some((target) => !labels.has(target.toLowerCase()))) return 'target_without_element';
  return undefined;
}

/**
 * Build the cua.jev_choice_request_v2 the provider receives; no tokens,
 * values, or pixels. A task that declares steps adds the progress counted
 * from this run's performed actions.
 */
export function nativeChoiceRequest(
  task: NativeTask,
  sources: TaskSources,
  step: NativeStep,
  history: readonly HistoryEntry[]
): Record<string, unknown> {
  const observation = sources.ax?.observation;
  if (!observation?.captureId) throw new Error('a native request needs an observation with a capture_id');
  const regions = (sources.visual?.observation.regions ?? []).map((region) =>
    task.redact({
      id: region.id,
      kind: region.kind,
      bounds: { x: region.x, y: region.y, width: region.width, height: region.height },
      text: region.text ?? null,
      label: region.label ?? null,
      confidence: region.confidence,
      interactive: region.interactive,
    })
  );
  return {
    schema: REQUEST_SCHEMA_V2,
    goal: task.redact(task.goal),
    capture_id: observation.captureId,
    snapshot_id: observation.snapshotId ?? null,
    regions,
    elements: step.elements.map((item) => task.redact(item)),
    history: history.slice(-MAX_HISTORY).map((item) => ({ selected_id: item.selected_id, outcome: item.outcome })),
    candidates: step.candidates.map((candidate) => ({
      id: candidate.id,
      description: task.redact(candidate.description + task.stepNote(candidate.id, history)),
      ...(candidate.source ? { source: candidate.source } : {}),
    })),
    ...(task.steps.length ? { progress: task.progress(history) } : {}),
  };
}

// Harness tasks. The same three tasks run on every repository harness that has
// a task mode: AppKit (macOS AX), WPF and WinUI3 (Windows UIA), and GTK3 (Linux AT-SPI).
// Each shows the same labeled controls and rewrites the same app-owned JSON
// state file, so task semantics, candidate IDs, and mock choices are identical
// across platforms. Only the window, the state schema, and the role table differ.

export type HarnessSpec = Readonly<{
  name: string;
  platform: 'macos' | 'windows' | 'linux';
  windowTitle: string;
  stateSchema: string;
  stateEnv: string;
  bundleId?: string;
  processName?: string;
}>;

export const HARNESSES: Readonly<Record<string, HarnessSpec>> = {
  appkit: {
    name: 'appkit',
    platform: 'macos',
    windowTitle: 'CuaTestHarness AppKit',
    stateSchema: 'cua.appkit_task_state_v1',
    stateEnv: 'CUA_APPKIT_TASK_STATE',
    bundleId: 'com.trycua.harness.appkit',
  },
  // WPF, WinUI3, and GTK3 show a dedicated task window in task mode: their
  // ordinary main windows scroll, so most controls would be off screen (and
  // excluded).
  wpf: {
    name: 'wpf',
    platform: 'windows',
    windowTitle: 'CuaTestHarness WPF Tasks',
    stateSchema: 'cua.wpf_task_state_v1',
    stateEnv: 'CUA_WPF_TASK_STATE',
    processName: 'CuaTestHarness.Wpf',
  },
  winui3: {
    name: 'winui3',
    platform: 'windows',
    windowTitle: 'CuaTestHarness WinUI3 Tasks',
    stateSchema: 'cua.winui3_task_state_v1',
    stateEnv: 'CUA_WINUI3_TASK_STATE',
    processName: 'CuaTestHarness.WinUI3',
  },
  gtk3: {
    name: 'gtk3',
    platform: 'linux',
    windowTitle: 'CuaTestHarness GTK3 Tasks',
    stateSchema: 'cua.gtk3_task_state_v1',
    stateEnv: 'CUA_GTK3_TASK_STATE',
    processName: 'python3',
  },
};

export const APPKIT_WINDOW_TITLE = HARNESSES.appkit.windowTitle;
export const APPKIT_BUNDLE_ID = HARNESSES.appkit.bundleId as string;
export const APPKIT_STATE_SCHEMA = HARNESSES.appkit.stateSchema;
export const COUNTER_TARGET = 3;
export const DEFAULT_NOTE_TEXT = 'jev-use native note';
export const TASK_KINDS = ['counter', 'save-note', 'choose-size'] as const;

// The cross-platform visual-only canvas has no accessibility tree, so it is not
// a form harness: it has one task, reached only through the visual fallback.
export type TaskHarness = Omit<HarnessSpec, 'platform'> & Readonly<{ platform: HarnessSpec['platform'] | 'any' }>;

export const CANVAS: TaskHarness = {
  name: 'canvas',
  platform: 'any',
  windowTitle: 'Cua Visual-Only Canvas Fixture',
  stateSchema: 'cua.visual_canvas_task_state_v1',
  stateEnv: 'CUA_CANVAS_TASK_STATE',
};
export const CANVAS_TASK_ID = 'canvas-cancel';
// Cancel, not Save: see python/native_tasks.py CANVAS_TARGET.
export const CANVAS_TARGET = 'Cancel';
// A painted surface has no deeper tree; see python/native_tasks.py CANVAS_MAX_DEPTH.
export const CANVAS_MAX_DEPTH = 1;
export const CANVAS_MIN_CONFIDENCE = 0.8;

export const harnessTaskIds = (harness: string): string[] => TASK_KINDS.map((kind) => `${harness}-${kind}`);
export const APPKIT_TASK_IDS = harnessTaskIds('appkit');
export const NATIVE_TASK_IDS = [...Object.keys(HARNESSES).flatMap(harnessTaskIds), CANVAS_TASK_ID];

type TaskKind = (typeof TASK_KINDS)[number] | 'cancel';

export function splitTaskId(taskId: string): [TaskHarness, TaskKind] {
  if (taskId === CANVAS_TASK_ID) return [CANVAS, 'cancel'];
  const index = taskId.indexOf('-');
  const harness = index > 0 ? HARNESSES[taskId.slice(0, index)] : undefined;
  const kind = taskId.slice(index + 1) as (typeof TASK_KINDS)[number];
  if (!harness || !TASK_KINDS.includes(kind)) throw new Error(`unknown native task: ${taskId}`);
  return [harness, kind];
}

export type HarnessTaskOptions = { pid?: number; noteText?: string; allowForeground?: boolean };

export function nativeTask(taskId: string, statePath: string, options: HarnessTaskOptions = {}): NativeTask {
  const [harness, kind] = splitTaskId(taskId);
  const oracle = new AppStateOracle(statePath, harness.stateSchema, options.pid);
  const scope: WindowScope = {
    windowTitle: harness.windowTitle,
    ...(harness.bundleId ? { bundleId: harness.bundleId } : {}),
    ...(harness.processName ? { processName: harness.processName } : {}),
  };
  const allowForeground = options.allowForeground ?? false;
  if (kind === 'cancel') {
    return new NativeTask({
      id: taskId,
      goal:
        'The window is a painted canvas with Save, Send, and Cancel cards. ' +
        `Click the ${CANVAS_TARGET} card once, then stop.`,
      scope: { ...scope, maxDepth: CANVAS_MAX_DEPTH },
      allowedActions: new Set<ActionKind>(['visual_click']),
      oracle,
      check: (state) => {
        if (state.selected === slug(CANVAS_TARGET) && state.action_count === 1) return 'verified';
        return state.selected != null ? 'refuted' : 'pending';
      },
      allowForeground,
      maxSteps: 4,
      visualTargets: [CANVAS_TARGET],
      visualMinConfidence: CANVAS_MIN_CONFIDENCE,
      mockPreferences: [`visual:${slug(CANVAS_TARGET)}`],
    });
  }
  if (kind === 'counter') {
    return new NativeTask({
      id: taskId,
      goal:
        `The counter starts at 0. Set it to exactly ${COUNTER_TARGET} by pressing ` +
        'Increment once per step, then stop.',
      scope,
      allowedActions: new Set<ActionKind>(['press']),
      oracle,
      check: (state) => {
        const counter = state.counter;
        if (counter === COUNTER_TARGET) return 'verified';
        return typeof counter === 'number' && counter > COUNTER_TARGET ? 'refuted' : 'pending';
      },
      allowForeground,
      maxSteps: COUNTER_TARGET + 3,
      mockPreferences: ['ax:button:increment'],
      steps: [
        { description: 'Press the button labeled "Increment"', candidateId: 'ax:button:increment', times: COUNTER_TARGET },
      ],
    });
  }
  if (kind === 'save-note') {
    const note = options.noteText ?? DEFAULT_NOTE_TEXT;
    return new NativeTask({
      id: taskId,
      goal: 'Enter the note text into the Note field, then save the note.',
      scope,
      allowedActions: new Set<ActionKind>(['press', 'set_text']),
      oracle,
      check: (state) =>
        state.note_saved === note ? 'verified' : state.note_saved != null ? 'refuted' : 'pending',
      parameters: [{ name: 'note', value: note, secret: true, redaction: '[note text]' }],
      allowForeground,
      maxSteps: 5,
      mockPreferences: ['ax:text_input:note:set:note', 'ax:button:save-note'],
      steps: [
        {
          description: 'Set the text field "Note" to the task parameter "note"',
          candidateId: 'ax:text_input:note:set:note',
        },
        { description: 'Press the button labeled "Save note"', candidateId: 'ax:button:save-note' },
      ],
    });
  }
  return new NativeTask({
    id: taskId,
    goal: 'Choose the Large size option and check the I agree checkbox.',
    scope,
    allowedActions: new Set<ActionKind>(['select', 'toggle']),
    oracle,
    check: (state) => (state.size === 'large' && state.agreed === true ? 'verified' : 'pending'),
    allowForeground,
    maxSteps: 5,
    mockPreferences: ['ax:radio:large', 'ax:checkbox:i-agree'],
    steps: [
      { description: 'Select the radio option "Large"', candidateId: 'ax:radio:large' },
      // The oracle accepts either order, so neither step waits for the other.
      { description: 'Toggle the checkbox "I agree"', candidateId: 'ax:checkbox:i-agree', afterPrevious: false },
    ],
  });
}

/** Build one AppKit harness task (kept for Phase 1 callers). */
export function appkitTask(taskId: string, statePath: string, options: HarnessTaskOptions = {}): NativeTask {
  if (!APPKIT_TASK_IDS.includes(taskId)) throw new Error(`unknown AppKit task: ${taskId}`);
  return nativeTask(taskId, statePath, options);
}

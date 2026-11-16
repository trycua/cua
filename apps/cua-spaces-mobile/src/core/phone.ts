// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Typed views of the app core's phone methods (`libs/cua/crates/cua-spaces-app-core/src/phone`). Every sentence a
 * person reads in the start-a-task screen, the outbox rows, approval buttons, Queue and Steer, dictation, diffs and
 * push settings is the core's; these wrappers only give the answers a type. Where the core is not linked (Android
 * until it is built there) each returns null and the screen says so, never a second set of words.
 */

import { core } from "./index";

/* ---- Start a task ------------------------------------------------------------------- */

export interface MachineOption {
  id: string;
  name: string;
  online: boolean;
  canCreate: boolean;
  why?: string | null;
  os: string[];
  /** The agents (harness ids) connected on this machine; null when it did not say. */
  readyAgents?: string[] | null;
}
export interface AgentOption {
  harness: string;
  label: string;
  ready: boolean;
  why?: string | null;
}
export interface SpaceOption {
  id: string;
  name: string;
  machineId: string;
  online: boolean;
  agents: { handle: string; harness: string; label: string }[];
}
export type WhereChoice = { kind: "auto" } | { kind: "machine"; machine: string } | { kind: "space"; space: string };
export interface TaskChoice {
  agent?: string | null;
  where?: WhereChoice | null;
  repo?: string | null;
}
export interface StartTaskInput {
  machines: MachineOption[];
  agents: AgentOption[];
  spaces: SpaceOption[];
  choice: TaskChoice;
}
export interface StartTaskView {
  title: string;
  placeholder: string;
  agentTitle: string;
  whereTitle: string;
  agents: { harness: string; label: string; selected: boolean; enabled: boolean; detail: string | null }[];
  wheres: { id: string; kind: "auto" | "machine" | "space"; label: string; detail: string | null; selected: boolean; enabled: boolean }[];
  repo: { label: string; set: boolean; placeholder: string; hint: string; clearLabel: string; problem: string | null };
  startLabel: string;
  cancelLabel: string;
  starting: { title: string; detail: string; failedTitle: string; retry: string; leave: string };
  agent: string | null;
  where: WhereChoice;
  blocker: string | null;
}
/** The `task_start` arguments, as the core builds them (snake_case: the daemon's wire). */
export interface TaskRequest {
  client_id: string;
  agent: string;
  model?: string;
  prompt: string;
  message: string;
  where: WhereChoice;
  os: string;
  repo?: { url: string };
}
export interface StartTaskPlan {
  ok: boolean;
  error: string | null;
  request: TaskRequest | null;
  queuedNote: string;
  title: string;
}

export const startTaskView = (input: StartTaskInput) => core.tryCall<StartTaskView>("startTask.view", { input });
export const startTaskPlan = (input: StartTaskInput, prompt: string, clientId: string, model?: string | null) =>
  core.tryCall<StartTaskPlan>("startTask.plan", { input, prompt, clientId, ...(model ? { model } : {}) });

/**
 * Example jobs for an empty box: the core's prompt library (`firstPrompts.choose`), read as it is, never copied. A phone
 * is not a host, so it asks as a machine that can make a Linux Space and nothing else (the plain, universal examples).
 * A core that does not have the library yet answers nothing, and the screen shows no cards.
 */
export interface PromptSuggestion {
  id: string;
  family: string;
  title: string;
  detail: string;
  os: "macos" | "windows" | "linux";
  /** What goes into the box; ends with ": " when the person fills in the rest. */
  prompt: string;
}
const PHONE_ENV = { defaultLocation: "auto", localAvailable: false, localReason: "A phone does not run Spaces.", maxCpus: 0 };
export const promptSuggestions = (): PromptSuggestion[] =>
  core.tryCall<{ suggestions: PromptSuggestion[] }>("firstPrompts.choose", { env: PHONE_ENV, hostOs: "linux" })?.suggestions ?? [];

/* ---- Tool rows ---------------------------------------------------------------------- */

export interface ToolCallIn {
  cat: string;
  line: string;
  detail?: string;
  state?: "running" | "done" | "failed";
}
export interface ToolGroupView {
  phrase: string | null;
  rows: { line: string; detail: string; state: "running" | "done" | "failed"; stateLabel: string | null }[];
  running: boolean;
  failed: boolean;
  showLabel: string;
  hideLabel: string;
  closedLabel: string;
}
export const toolGroup = (calls: ToolCallIn[]) => core.tryCall<ToolGroupView>("toolRows.group", { calls });

/* ---- The outbox --------------------------------------------------------------------- */

export interface OutMessage {
  id: string;
  space: string;
  kind: string;
  text: string;
  replyTo: number | null;
  mode: "send" | "queue" | "steer";
  extra: Record<string, unknown> | null;
  state: "queued" | "delivering" | "delivered" | "failed";
  attempts: number;
  sentMs: number;
  claimedMs: number | null;
  deliveredMs: number | null;
  retryAtMs: number | null;
  error: string | null;
}
export interface OutboxState {
  schema: number;
  online: boolean;
  lost: number;
  messages: OutMessage[];
}
export type OutboxAction =
  | {
      type: "send";
      nowMs: number;
      id: string;
      space: string;
      kind?: string;
      text?: string;
      replyTo?: number | null;
      mode?: string;
      extra?: Record<string, unknown> | null;
    }
  | { type: "online"; nowMs: number; online: boolean }
  | { type: "tick"; nowMs: number }
  | { type: "delivered"; nowMs: number; id: string }
  | { type: "deliveryFailed"; nowMs: number; id: string; network?: boolean; error?: string }
  | { type: "resume"; nowMs: number }
  | { type: "threadHas"; nowMs: number; id: string; found: boolean }
  | { type: "retry"; nowMs: number; id: string }
  | { type: "discard"; nowMs: number; id: string };
export type OutboxEffect = { type: "deliver"; id: string } | { type: "verifyDelivery"; id: string; space: string } | { type: "wakeAt"; atMs: number };
export interface OutboxRow {
  id: string;
  space: string;
  kind: string;
  text: string;
  state: "waiting" | "sending" | "failed";
  label: string;
  canRetry: boolean;
  canDiscard: boolean;
  retryLabel: string;
  discardLabel: string;
  mode: string;
}
export interface OutboxView {
  rows: OutboxRow[];
  banner: string | null;
  waiting: number;
  failed: number;
}
export interface EntryLite {
  id: string;
  clientId?: string | null;
  authorId: string;
  text: string;
  tsMs: number;
}

export const outboxInitial = (): OutboxState | null => core.tryCall<OutboxState>("outbox.initial");
export const outboxReduce = (state: OutboxState, action: OutboxAction) =>
  core.tryCall<{ state: OutboxState; effects: OutboxEffect[] }>("outbox.reduce", { state, action });
export const outboxView = (state: OutboxState, space?: string | null) => core.tryCall<OutboxView>("outbox.view", { state, ...(space ? { space } : {}) });
/** The line over saved content while offline ("You're offline. Showing what was saved 2h ago."); null while online. */
export const cacheLine = (savedMs: number | null, nowMs: number, offline: boolean) => core.tryCall<string | null>("cache.line", { savedMs, nowMs, offline });
export const outboxFound = (message: OutMessage, entries: EntryLite[], me: string) => core.tryCall<boolean>("outbox.found", { message, entries, me });

/* ---- Approvals, Queue and Steer, dictation ------------------------------------------ */

export interface PendingApproval {
  seq: number;
  kind: string;
  action: string;
  scopeChoices: string[];
}
export interface ApprovalButton {
  id: "decline" | "allow-once" | "allow-space";
  label: string;
  approve: boolean;
  scope: "once" | "space" | null;
  role: "primary" | "secondary" | "quiet";
  hint: string | null;
}
export interface ApprovalsView {
  counter: string | null;
  index: number;
  total: number;
  current: number | null;
  next: number | null;
  previous: number | null;
  nextLabel: string | null;
  previousLabel: string | null;
  buttons: ApprovalButton[];
  scopeNote: string | null;
}
export const approvalsView = (input: { pending: PendingApproval[]; current?: number | null; allowLabel?: string | null; denyLabel?: string | null }) =>
  core.tryCall<ApprovalsView>("approvals.view", { input });
export const approvalResult = (approve: boolean, scope: string | null, title: string) => core.tryCall<string>("approvals.result", { approve, scope, title });

export interface SendChoice {
  mode: "send" | "queue" | "steer";
  label: string;
  hint: string;
}
export interface SendModeView {
  primary: SendChoice;
  alternate: SendChoice | null;
  queuedLine: string | null;
  accessibilityLabel: string;
  stop: boolean;
  stopLabel: string;
}
export const sendModeView = (input: { running: boolean; hasText: boolean; queued?: number; canSteer?: boolean }) =>
  core.tryCall<SendModeView>("sendMode.view", { input });

export type PermissionWord = "undetermined" | "granted" | "denied" | "restricted";
export interface VoiceView {
  available: boolean;
  canStart: boolean;
  buttonLabel: string;
  buttonHint: string;
  status: string | null;
  notice: { text: string; action: "open-settings" | "retry" | null; actionLabel: string | null } | null;
  privacyNote: string | null;
  willAsk: boolean;
}
export interface VoiceInput {
  microphone: PermissionWord;
  speech: PermissionWord;
  recognizerAvailable: boolean;
  onDevice: boolean;
  listening: boolean;
  error?: string | null;
}
export const voiceView = (input: VoiceInput) => core.tryCall<VoiceView>("voice.view", { input });
export const voiceInsert = (text: string, start: number, end: number, transcript: string) =>
  core.tryCall<{ text: string; cursor: number }>("voice.insert", { text, start, end, transcript });

/* ---- Push --------------------------------------------------------------------------- */

export interface PushRoute {
  target: "approval" | "thread" | "home";
  spaceId: string | null;
  entry: number | null;
  approval: number | null;
  link: string;
}
export const pushRoute = (data: unknown) => core.tryCall<PushRoute>("push.route", { data });

export interface PushRegistrationInput {
  permission: string;
  token: string | null;
  deviceId: string;
  env: "sandbox" | "production";
  appId: string;
  locale: string;
  needsYou: boolean;
  done: boolean;
  registered: string | null;
}
export interface PushRegistration {
  action: "register" | "unregister" | "none";
  method: string | null;
  path: string | null;
  body: Record<string, unknown> | null;
  fingerprint: string | null;
  reason: string | null;
}
export const pushRegistration = (input: PushRegistrationInput) => core.tryCall<PushRegistration>("push.registration", { input });

export interface PushSettingsView {
  title: string;
  rows: { id: "needsYou" | "done"; label: string; detail: string; on: boolean; accessibilityLabel: string }[];
  status: { text: string; tone: "ok" | "warn" | "muted"; action: "allow" | "open-settings" | null; actionLabel: string | null };
  footer: string;
}
export const pushSettingsView = (input: { permission: string; needsYou: boolean; done: boolean; registered: boolean; pushAvailable: boolean }) =>
  core.tryCall<PushSettingsView>("push.settings", { input });

/* ---- Diffs -------------------------------------------------------------------------- */

export interface FileChange {
  path: string;
  oldPath?: string | null;
  status: string;
  additions?: number | null;
  deletions?: number | null;
  binary?: boolean;
  size?: number | null;
  mime?: string | null;
}
export interface ChangesView {
  title: string;
  repo: string;
  branchLine: string;
  filesTotal: string;
  additions: string;
  deletions: string;
  files: {
    path: string;
    name: string;
    dir: string;
    status: string;
    statusLabel: string;
    additions: string;
    deletions: string;
    note: string;
    bar: ("add" | "del" | "none")[];
    binary: boolean;
    preview: "image" | "text" | "none";
  }[];
  truncatedNote: string | null;
  empty: string | null;
  verifiedNote: string | null;
}
export interface DiffLabels {
  open: string;
  title: string;
  diffTab: string;
  previewTab: string;
  loading: string;
  loadingFile: string;
  unavailable: string;
  retry: string;
  back: string;
  imageAlt: string;
}
export const diffLabels = () => core.tryCall<DiffLabels>("diff.labels");

export const diffChanges = (input: {
  repo: string;
  branch: string;
  base?: string | null;
  headSha?: string | null;
  files: FileChange[];
  truncated?: boolean;
  verified?: boolean;
}) => core.tryCall<ChangesView>("diff.changes", { input });

export interface Segment {
  text: string;
  changed: boolean;
}
export interface DiffLine {
  kind: "context" | "add" | "del";
  oldNo: number | null;
  newNo: number | null;
  text: string;
  segments: Segment[];
  noNewline: boolean;
}
export interface FileDiffView {
  path: string;
  name: string;
  dir: string;
  hunks: { header: string; lines: DiffLine[] }[];
  additions: string;
  deletions: string;
  notes: string[];
  unifiedLabel: string;
  wordsLabel: string;
  hasWords: boolean;
}
export const diffFile = (input: { path: string; patch: string; truncated?: boolean; binary?: boolean }) => core.tryCall<FileDiffView>("diff.file", { input });

export interface PreviewView {
  kind: "image" | "text" | "none";
  name: string;
  sizeLabel: string | null;
  note: string | null;
}
export const diffPreview = (input: { path: string; mime?: string | null; size?: number | null; binary?: boolean; truncated?: boolean }) =>
  core.tryCall<PreviewView>("diff.preview", { input });

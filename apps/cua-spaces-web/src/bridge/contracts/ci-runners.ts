// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The CI Runners page: types mirroring `cua-spaces-app-core/src/ci_runners.rs`
 * (the page's view model, state and actions) and `cua-ci`'s `overview` module
 * (what `cua ci overview` prints, in snake_case).
 */

/* ---- What `cua ci overview` prints (only what the page reads) ------------------------------ */

export interface CiSetupStep {
  id: "connect_github" | "repositories" | "runner_label" | "first_job_passed" | string;
  state: "done" | "todo" | "unknown" | string;
  detail: string;
}

export interface CiGithub {
  connected: boolean;
  /** `hosted` (the Cua CI GitHub app), `own` or `none`. */
  mode: string;
  login: string;
  choose_url: string;
  note?: string;
}

export interface CiRepoUse {
  name: string;
  jobs_7d: number;
}

export interface CiRepoChoice {
  name: string;
  allowed: boolean;
}

export interface CiJob {
  job_key: string;
  name: string;
  repo: string;
  state: "queued" | "running" | "passed" | "failed" | string;
  queued_ms: number;
  wait_ms?: number | null;
  run_ms?: number | null;
  cost_micros?: number | null;
  all_in_micros?: number | null;
  start_mode: string;
  machine: string;
  machine_type: string;
  cloud: string;
  spot: boolean;
  log_pool: string;
  log_slot: string;
}

export interface CiPilotClass {
  class: string;
  phase: string;
  jobs_seen: number;
  warm_now: number;
  idle_ttl_secs: number;
  spot: boolean;
  reasons: string[];
}

export interface CiTarget {
  target: string;
  goal: string;
  warm?: number | null;
  autopilot?: { classes: CiPilotClass[] } | null;
}

export interface CiOverview {
  generated_ms: number;
  setup: CiSetupStep[];
  github: CiGithub;
  repositories: { used: CiRepoUse[]; all: CiRepoChoice[] | null };
  usage: { starts_24h: number; jobs_7d: number; pass_rate_7d: number | null; spend_7d_micros: number };
  jobs: CiJob[];
  targets: CiTarget[];
  budget: { month_committed_micros: number; monthly_micros: number; state: string };
}

/* ---- What `cua ci app <op>` answers --------------------------------------------------------- */

export type CiGoal = "cheapest" | "balanced" | "fastest";

export interface CiConnectAnswer {
  /** `waiting` (open `url`), `connected`, `sign_in`, `failed`. */
  state: "waiting" | "connected" | "sign_in" | "failed";
  url?: string | null;
  message?: string | null;
  login?: string;
}

export interface CiLogLine {
  at_ms: number;
  text: string;
}

/* ---- The page's state and actions ----------------------------------------------------------- */

export type CiTab = "all" | "running" | "failed";
export type CiPhase = "closed" | "choose" | "waiting" | "token";

export interface CiConnectState {
  phase: CiPhase;
  busy: boolean;
  url: string | null;
  org: string;
  error: string | null;
}

export interface CiState {
  tab: CiTab;
  expanded: string | null;
  picker: boolean;
  query: string;
  help: string | null;
  connect: CiConnectState;
}

export type CiAction =
  | { type: "tab"; tab: CiTab }
  | { type: "expand"; key: string }
  | { type: "picker"; open: boolean }
  | { type: "query"; value: string }
  | { type: "help"; id: string }
  | { type: "connectOpen" }
  | { type: "connectClose" }
  | { type: "connectStart" }
  | { type: "connectAnswer"; answer: { state: string; url?: string | null; message?: string | null } }
  | { type: "useToken" }
  | { type: "tokenBack" }
  | { type: "tokenOrg"; value: string }
  | { type: "tokenSubmit" }
  | { type: "tokenDone"; ok: boolean; message?: string | null };

/* ---- The view ---------------------------------------------------------------------------------- */

export interface CiButton {
  id: string;
  label: string;
}

export interface CiSetupRow {
  id: string;
  title: string;
  detail: string;
  state: "done" | "todo" | "pending";
  action: CiButton | null;
}

export interface CiSetupView {
  title: string;
  rows: CiSetupRow[];
  progress: string;
}

export interface CiStat {
  label: string;
  value: string;
  tone: "warn" | "bad" | null;
}

export interface CiTabView {
  id: CiTab;
  label: string;
  count: number;
  selected: boolean;
}

export interface CiFact {
  label: string;
  value: string;
}

export interface CiTimeline {
  waitPct: number;
  runPct: number;
  waitLabel: string;
  runLabel: string;
}

export interface CiLogRef {
  jobKey: string;
  pool: string;
  slot: string;
}

export interface CiJobDetail {
  timeline: CiTimeline | null;
  facts: CiFact[];
  openLabel: string;
  openUrl: string | null;
  logLabel: string;
  log: CiLogRef;
}

export interface CiJobRowView {
  key: string;
  title: string;
  repo: string;
  state: string;
  stateLabel: string;
  wait: string;
  run: string;
  cost: string;
  when: string;
  expanded: boolean;
  detail: CiJobDetail | null;
}

export interface CiJobsView {
  title: string;
  tabs: CiTabView[];
  columns: string[];
  rows: CiJobRowView[];
  empty: string;
}

export interface CiRepoRowView {
  name: string;
  status: string;
  jobs: string;
  allowed: boolean;
  canToggle: boolean;
}

export interface CiPickerView {
  title: string;
  summary: string;
  searchLabel: string;
  rows: CiRepoRowView[];
  empty: string;
  chooseLabel: string;
  chooseUrl: string | null;
  connectLabel: string;
}

export interface CiGoalOption {
  id: CiGoal;
  label: string;
  selected: boolean;
}

export interface CiPilotLine {
  label: string;
  line: string;
}

export interface CiAutopilotView {
  target: string;
  title: string;
  goals: CiGoalOption[];
  goalNote: string;
  budget: string;
  budgetTone: "warn" | "bad" | null;
  plans: CiPilotLine[];
  reasons: string[];
}

export interface CiHelpTopic {
  id: string;
  title: string;
  body: string | null;
  open: boolean;
}

export interface CiTokenForm {
  orgLabel: string;
  orgPlaceholder: string;
  org: string;
  tokenLabel: string;
  tokenHelp: string;
  createLabel: string;
  createUrl: string;
  canSubmit: boolean;
}

export interface CiConnectView {
  phase: "choose" | "waiting" | "token";
  title: string;
  body: string;
  primary: string;
  busy: boolean;
  secondary: string | null;
  close: string;
  url: string | null;
  error: string | null;
  token: CiTokenForm | null;
}

export interface CiView {
  title: string;
  intro: string;
  connectButton: CiButton | null;
  setup: CiSetupView | null;
  stats: CiStat[];
  jobs: CiJobsView;
  repositories: CiPickerView;
  pickerOpen: boolean;
  sizingTitle: string;
  cloudLabel: string;
  sizing: CiAutopilotView[];
  noCloud: string;
  helpTitle: string;
  help: CiHelpTopic[];
  connect: CiConnectView | null;
  note: string | null;
  runsOn: string;
}

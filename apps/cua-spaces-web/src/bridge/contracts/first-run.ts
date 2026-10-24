// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The 0.10.0 first run: hand-written mirror of
 * `libs/cua/crates/cua-spaces-app-core/src/first_run.rs` (camelCase JSON).
 * Rust is the source of truth.
 */

export type FirstRunStep = "connect" | "setup";

export interface FirstRunAgent {
  agent: string;
  name: string;
  installed: boolean;
  configured: boolean;
  /** The account it signs in with on its own (`ChatGPT` for Codex). */
  signIn?: string | null;
  signedIn?: boolean;
}

export interface FirstRunPermission {
  id: string;
  granted: boolean;
  settingsUrl?: string | null;
}

export interface FirstRunHost {
  configured: boolean;
  running: boolean;
  online?: boolean | null;
  permissions: FirstRunPermission[];
  error?: string | null;
}

export interface FirstRunOffer {
  os: string;
  time?: string | null;
  size?: string | null;
  blocked?: string | null;
}

export type FirstRunExtraPhase = "none" | "not-now" | "creating" | "ready" | "failed";

export interface FirstRunExtra {
  phase: FirstRunExtraPhase;
  name?: string | null;
  os?: string | null;
  detail?: string | null;
  fraction?: number | null;
  error?: string | null;
}

export interface FirstRunPlan {
  metered: boolean;
  plan: string;
  sessionsLimit?: number | null;
  hoursLimit?: number | null;
  teammates: boolean;
  dismissed: boolean;
  accountApp: string;
}

export interface FirstRunInput {
  step: FirstRunStep;
  hostOs: string;
  agents: FirstRunAgent[] | null;
  agent: string | null;
  /** Step 1: the ticks (null: not changed, so the installed ones). Step 2: the agents connected. */
  picked: string[] | null;
  /** The provider keys saved on this computer (null: not read, or this app can't). */
  keys: { keys: { provider: string; env: string; last4: string; addedMs: number }[]; unavailable?: string | null; error?: string | null } | null;
  /** The agent whose own sign-in is waiting for the browser. */
  agentSigningIn: string | null;
  agentSignInError: string | null;
  connecting: boolean;
  connectError: string | null;
  agentOpenBeforeSetup: boolean | null;
  agentUsedAt: string | null;
  signedIn: boolean;
  signingIn: boolean;
  host: FirstRunHost | null;
  hostSettingUp: boolean;
  offers: FirstRunOffer[];
  extra: FirstRunExtra;
  plan: FirstRunPlan | null;
}

export interface FirstRunButton {
  id: string;
  label: string;
}

/** An agent on step 1. */
export interface FirstRunAgentPick {
  agent: string;
  name: string;
  checked: boolean;
  installed: boolean;
  /** Cua is in its settings already: it stays ticked. */
  locked: boolean;
  note: string | null;
}

/** A sign-in or a provider key on step 1. */
export interface FirstRunCredential {
  /** `sign-in:codex`, `key:anthropic`, `key:openai`. */
  id: string;
  title: string;
  note: string;
  status: string | null;
  /** The status is a done one ("Signed in", "Saved"), not a wait. */
  ok: boolean;
  /** `agent-sign-in` (with `agent`) or `add-key` (with `provider`). */
  action: FirstRunButton | null;
  agent: string | null;
  provider: string | null;
  error: string | null;
}

export interface FirstRunConnectView {
  title: string;
  accent: string;
  lede: string;
  intro: string;
  agentsLabel: string;
  agents: FirstRunAgentPick[];
  moreLabel: string;
  more: FirstRunAgentPick[];
  agentsNote: string;
  credentialsLabel: string;
  credentialsNote: string;
  credentials: FirstRunCredential[];
  primary: FirstRunButton;
  /** The agents Continue connects. */
  connects: string[];
  busy: string | null;
  error: string | null;
}

export type FirstRunRowState = "checking" | "ok" | "fix";

export interface FirstRunRow {
  id: "agent" | "computer";
  state: FirstRunRowState;
  title: string;
  note: string;
  action: FirstRunButton | null;
  settingsUrl: string | null;
}

export interface FirstRunPromptView {
  heading: string;
  text: string;
  note: string;
  copy: string | null;
}

export interface FirstRunExtraChoice {
  os: string;
  label: string;
  detail: string;
  blocked: string | null;
}

export interface FirstRunExtraView {
  phase: "choose" | "creating" | "ready" | "failed" | "not-now";
  title: string;
  tag: string | null;
  note: string;
  choices: FirstRunExtraChoice[];
  action: FirstRunButton | null;
  fraction: number | null;
  footnote: string | null;
}

export interface FirstRunUpsell {
  plan: "pro" | "teams";
  title: string;
  lines: string[];
  price: string;
  cta: FirstRunButton;
  url: string;
  dismiss: string;
}

export interface FirstRunPrimary {
  id: "done" | "finish-later";
  label: string;
  enabled: boolean;
  note: string | null;
}

export interface FirstRunSetupView {
  title: string;
  rows: FirstRunRow[];
  /** None when no agent was picked. */
  prompt: FirstRunPromptView | null;
  extra: FirstRunExtraView;
  upsell: FirstRunUpsell | null;
  primary: FirstRunPrimary;
}

export interface FirstRunArt {
  agent: string;
  computer: "on" | "faded";
  extra: "none" | "outline" | "filling" | "on";
}

export interface FirstRunView {
  step: FirstRunStep;
  later: string;
  connect: FirstRunConnectView | null;
  setup: FirstRunSetupView | null;
  art: FirstRunArt;
  dots: number;
  current: number;
}

export interface FirstRunTodo {
  id: "agent" | "computer" | "prompt" | "extra" | "sign-in";
  title: string;
  note: string;
  done: boolean;
  optional: boolean;
  action: FirstRunButton | null;
  settingsUrl: string | null;
}

export interface FirstRunTodos {
  title: string;
  lede: string;
  progress: string;
  items: FirstRunTodo[];
  complete: boolean;
}

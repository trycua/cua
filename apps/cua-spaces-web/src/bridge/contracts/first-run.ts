// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The 0.10.0 first run: hand-written mirror of
 * `libs/cua/crates/cua-spaces-app-core/src/first_run.rs` (camelCase JSON).
 * Rust is the source of truth.
 */

/** Welcome, Pick your agent (`connect`), Connect your other devices, Try it (`setup`). */
export type FirstRunStep = "welcome" | "connect" | "devices" | "setup";

export interface FirstRunAgent {
  agent: string;
  name: string;
  installed: boolean;
  configured: boolean;
  /** The account it signs in with on its own (`ChatGPT` for Codex). */
  signIn?: string | null;
  signedIn?: boolean;
  /** The settings file Cua adds itself to. */
  mcpConfig?: string | null;
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
  /** The agents connected on Pick your agent (an agent the setup reports configured counts either way). */
  picked: string[] | null;
  /** The provider keys saved on this computer (null: not read, or this app can't). */
  keys: { keys: { provider: string; env: string; last4: string; addedMs: number }[]; unavailable?: string | null; error?: string | null } | null;
  /** The agent whose own sign-in is waiting for the browser. */
  agentSigningIn: string | null;
  agentSignInError: string | null;
  connecting: boolean;
  /** The agent being connected. */
  connectingAgent?: string | null;
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
  /** The file Connect changes (`~/.claude.json`). */
  file: string | null;
  /** Cua is in its settings. */
  connected: boolean;
  /** "Connect" (`connect`), none once connected or while it runs. */
  action: FirstRunButton | null;
  /** "Connected", "Adding Cua…". */
  note: string | null;
}

/** No agent found: install one, or use the CLI. */
export interface FirstRunAlternative {
  id: "install" | "cli";
  title: string;
  note: string;
  url: string | null;
  command: string | null;
}

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

export interface FirstRunWelcomeView {
  title: string;
  lede: string;
  primary: FirstRunButton;
}

export interface FirstRunConnectView {
  title: string;
  lede: string;
  /** The agents found here, most preferred first. */
  agents: FirstRunAgentPick[];
  alternatives: FirstRunAlternative[];
  /** The connected agents' sign-ins and keys, in the agents' order. */
  credentials: FirstRunCredential[];
  credentialsNote: string;
  primary: FirstRunButton;
  /** "Skip", until an agent is connected. */
  secondary: FirstRunButton | null;
  error: string | null;
}

export interface FirstRunDevicesView {
  title: string;
  lede: string;
  rows: { title: string; note: string }[];
  /** The sign-in waiting, or this computer being added to the account. */
  status: FirstRunRow | null;
  /** "Sign in to connect" (`sign-in`), or "Continue" once signed in. */
  primary: FirstRunButton;
  /** "Not now" (`not-now`). */
  secondary: FirstRunButton | null;
  note: string | null;
}

export type FirstRunRowState = "checking" | "ok" | "fix";

export interface FirstRunRow {
  id: "agent" | "computer" | "account";
  state: FirstRunRowState;
  title: string;
  note: string;
  action: FirstRunButton | null;
  settingsUrl: string | null;
}

export interface FirstRunPromptTab {
  /** `macos`, `linux`, `omarchy`. */
  id: string;
  /** "macOS", "Linux", "Omarchy". */
  label: string;
  text: string;
}

export interface FirstRunPromptView {
  heading: string;
  /** The selected tab's request (or the only one). */
  text: string;
  note: string;
  copy: string | null;
  /** The request per system a first Space can run here (macOS only on a Mac). */
  tabs?: FirstRunPromptTab[];
  /** The tab `text` shows. */
  selected?: string | null;
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

export interface FirstRunPrimary {
  id: "done";
  label: string;
  enabled: boolean;
  note: string | null;
}

export interface FirstRunSetupView {
  /** "Try it in Hermes.", "Try a first Space." */
  title: string;
  lede: string;
  /** None with no connected agent. */
  prompt: FirstRunPromptView | null;
  /** "Waiting for Hermes…", then "Hermes used Cua at 18:12." */
  status: FirstRunRow | null;
  /** The one-click first Space, with no connected agent. */
  extra: FirstRunExtraView | null;
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
  welcome: FirstRunWelcomeView | null;
  connect: FirstRunConnectView | null;
  devices: FirstRunDevicesView | null;
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

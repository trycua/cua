// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Connections and summoning agents in a Space: the
 * app core's `connectors.*` and `summon.*` (`connectors.rs`, `summon.rs`)
 * over what the host already answers (`agents.setup`, `agentKeys.list`).
 * Every word, state and rule is the core's; this file only shapes inputs
 * and calls it.
 */

import type { CoreClient } from "./core";
import type { AgentSetupRow } from "./contracts/agents";
import { hostOs, type HostOs } from "./host-os";
import type { AgentKeysInput, AgentKeyRowView } from "./agent-keys";

type Json = Record<string, unknown>;

/* ---- Connections --------------------------------------------------------------- */

export interface ConnectorAppInput {
  agent: string;
  name: string;
  installed: boolean;
  configured: boolean;
  skills: boolean;
  mcp: boolean | null;
  mcpConfig: string | null;
}

export interface ConnectorsInput {
  apps: ConnectorAppInput[];
  /** The first key saved for each variable. */
  keys: Omit<AgentKeysInput, "extras" | "uses">;
  /** Keys saved beside the first for the same variable. */
  keyExtras?: NonNullable<AgentKeysInput["extras"]>;
  /** Which credential each app that takes any provider uses, by app. */
  uses?: NonNullable<AgentKeysInput["uses"]>;
  shellKeys: string[];
  auto: boolean;
  off: string[];
  busy: { id: string; step: "connecting" | "signing-in" | "signing-out" }[];
  failures: { id: string; line: string }[];
  /** What is typed in the search of More providers. */
  query?: string;
  /** More providers is open (a search opens it too). */
  moreOpen?: boolean;
}

export interface ConnectorButton {
  id: "connect" | "sign-out" | "sign-in" | "try-again";
  label: string;
  enabled: boolean;
}

export interface ConnectorAppRow {
  agent: string;
  name: string;
  state: "connected" | "not-connected" | "connecting" | "failed";
  line: string | null;
  button: ConnectorButton | null;
  connectedLabel: string | null;
  openLabel: string;
}

/** An app a connection works with: a chip that opens the app's page. */
export interface WorkChip {
  /** The app's setup id (`claude-code`, `codex`, `hermes`). */
  agent: string;
  name: string;
}

export interface ConnectorProviderRow {
  id: string;
  label: string;
  /** The icon's key (the page draws a mark of the initial when it has none). */
  icon: string;
  tier: "top" | "more";
  state: "connected" | "not-connected" | "connecting" | "failed";
  /** "Connected", "Needs reconnect", "Not connected", what is happening, or why it failed. */
  status: string;
  /** What is saved, when connected ("API key •••• 3f9a"). */
  credential: string | null;
  worksWith: WorkChip[];
  worksWithLabel: string;
  button: ConnectorButton;
  addedMs: number | null;
  signOutEnvs: string[];
}

export interface ConnectorsView {
  appsTitle: string;
  apps: ConnectorAppRow[];
  appsEmpty: string | null;
  /** Apps that are not on this machine and cannot run as agents (under Show all, with where to get them). */
  more: { agent: string; name: string }[];
  autoLabel: string;
  autoHelp: string;
  auto: boolean;
  providersTitle: string;
  providersFooter: string;
  /** Up front, then any other that is connected or needs reconnecting. */
  providers: ConnectorProviderRow[];
  moreTitle: string;
  moreSearchLabel: string;
  /** The searchable list (empty while More providers is closed). */
  moreProviders: ConnectorProviderRow[];
  moreCount: number;
  moreEmpty: string | null;
  others: AgentKeyRowView[];
  addOtherLabel: string;
  otherHelp: string;
  notice: string | null;
  canEdit: boolean;
  addedLabel: string;
}

/** The words around importing Claude Code's sign-in (`connectors.importSheet`). */
export interface ImportSheetView {
  title: string;
  lede: string;
  /** What is read, what is kept, where it goes. */
  points: string[];
  confirmLabel: string;
  cancelLabel: string;
  readingLabel: string;
  notFoundTitle: string;
  notFoundLede: string;
  /** In order: sign in with Claude Code, paste a token, use an API key. */
  options: { id: "login" | "token" | "key"; label: string; help: string; command: string | null; button: string }[];
}

export interface ConnectPlan {
  tools: boolean;
  appToken: boolean;
  device: boolean;
  /** Open this provider's sheet: the first the app can use, with "Other providers" in it. */
  provider: string | null;
  tokenProvider: string | null;
}

export interface FieldOption {
  id: string;
  label: string;
}

/** One field a way asks for besides the key: `region` is the plan or region, the others are `fields`. */
export interface WayField {
  id: string;
  label: string;
  kind: "text" | "url" | "choice";
  placeholder: string;
  required: boolean;
  options: FieldOption[];
  value: string;
}

export interface ProviderWay {
  id: "app-token" | "app-device" | "token" | "key" | "endpoint" | "shell";
  /** The method kind from the provider file. */
  kind: string;
  method: string;
  label: string;
  help: string;
  button: string;
  app: string | null;
  env: string | null;
  /** `required` (a key or token is pasted), `optional` or `none`. */
  secret: "required" | "optional" | "none";
  fields: WayField[];
}

export interface ProviderSheetView {
  provider: string;
  icon: string;
  title: string;
  /** "Works with Claude Code, Hermes and OpenCode." */
  lede: string;
  worksWith: WorkChip[];
  ways: ProviderWay[];
  cancelLabel: string;
  keyLabel: string;
  keyPlaceholder: string;
  keyHelp: string;
  tokenLabel: string;
  tokenPlaceholder: string;
  saveLabel: string;
  /** Read before saving (a server on this machine). */
  note: string | null;
  /** "Other providers": opens the provider choice. */
  switchLabel: string;
  /** Why only a key is offered, when this app can't use the provider's sign-in. */
  onlyKeyNote: string | null;
}

export interface ProviderChoice {
  provider: string;
  label: string;
  icon: string;
  tier: "top" | "more";
  state: "connected" | "not-connected" | "connecting" | "failed";
  status: string;
}

export interface ProviderPickerView {
  title: string;
  lede: string;
  searchLabel: string;
  items: ProviderChoice[];
  moreLabel: string | null;
  moreCount: number;
  empty: string | null;
  cancelLabel: string;
}

export interface ConnectorToggle {
  id: "skills" | "mcp" | "auto";
  label: string;
  help: string;
  on: boolean;
  enabled: boolean;
}

/** One credential an app can use (a row of the Uses menu). */
export interface UseOption {
  provider: string;
  providerName: string;
  icon: string;
  env: string;
  slot: string | null;
  /** "API key ending 4f2a", or the key's own name. */
  line: string;
  model: string | null;
  selected: boolean;
}

/** An app's Uses row: which saved credential it runs with, and the others it could. */
export interface AppUses {
  title: string;
  state: "connected" | "not-connected" | "gone";
  status: string;
  current: UseOption | null;
  options: UseOption[];
  chosen: boolean;
  connectLabel: string;
  note: string;
  enabled: boolean;
}

export interface AppDetailView {
  agent: string;
  name: string;
  backLabel: string;
  status: string;
  connected: boolean;
  connect: ConnectorButton | null;
  toolsTitle: string;
  toggles: ConnectorToggle[];
  signInTitle: string;
  /** Which credential the app uses (an app that takes any provider); it replaces `signIn`. */
  uses: AppUses | null;
  signIn: { title: string; status: string; connected: boolean; way: string; provider: string; button: ConnectorButton } | null;
  signInNote: string | null;
  settingsFileLabel: string;
  settingsFile: string | null;
  removeLabel: string | null;
}

export interface SetupCardView {
  title: string;
  body: string;
  connectLabel: string;
  connectingLabel: string;
  connectedLabel: string;
  retryLabel: string;
  changeLabel: string;
  detailsLabel: string;
}

/** The connectors input from the host's answers and the page's own state. */
export function connectorsInputOf(
  rows: readonly AgentSetupRow[] | null,
  keys: AgentKeysInput,
  extra: Partial<Omit<ConnectorsInput, "apps" | "keys">> = {},
): ConnectorsInput {
  return {
    apps: (rows ?? []).map((r) => ({
      agent: r.agent,
      name: r.name,
      installed: r.installed,
      configured: r.configured,
      skills: r.skillsTotal > 0 ? r.skillsInstalled >= r.skillsTotal : r.configured,
      mcp: r.mcpConfig ? r.configured || /MCP/i.test(r.detail) : null,
      mcpConfig: r.mcpConfig,
    })),
    keys: { keys: keys.keys, unavailable: keys.unavailable, error: keys.error },
    ...(keys.extras?.length ? { keyExtras: keys.extras } : {}),
    ...(keys.uses && Object.keys(keys.uses).length ? { uses: keys.uses } : {}),
    shellKeys: extra.shellKeys ?? [],
    auto: extra.auto ?? false,
    off: extra.off ?? [],
    busy: extra.busy ?? [],
    failures: extra.failures ?? [],
    query: extra.query ?? "",
    moreOpen: extra.moreOpen ?? false,
  };
}

export const connectorsView = (core: CoreClient, input: ConnectorsInput, os: HostOs = hostOs()): ConnectorsView | null =>
  core.tryCall<ConnectorsView>("connectors.view", { input: input as unknown as Json, hostOs: os }) ?? null;

export const connectPlan = (core: CoreClient, input: ConnectorsInput, agent: string): ConnectPlan | null =>
  core.tryCall<ConnectPlan>("connectors.connectPlan", { input: input as unknown as Json, agent }) ?? null;

export const providerSheet = (core: CoreClient, input: ConnectorsInput, provider: string, os: HostOs = hostOs(), forApp: string | null = null): ProviderSheetView | null =>
  core.tryCall<ProviderSheetView | null>("connectors.providerSheet", { input: input as unknown as Json, provider, hostOs: os, forApp }) ?? null;

export const importSheet = (core: CoreClient, os: HostOs = hostOs()): ImportSheetView | null =>
  core.tryCall<ImportSheetView>("connectors.importSheet", { hostOs: os }) ?? null;

export const providerPicker = (core: CoreClient, input: ConnectorsInput, forApp: string | null = null, query = "", expanded = false): ProviderPickerView | null =>
  core.tryCall<ProviderPickerView>("connectors.providerPicker", { input: input as unknown as Json, forApp, query, expanded }) ?? null;

/** One model row of the prompt box's model menu. */
export interface ModelChoice {
  id: string;
  label: string;
  /** A quick, cheap model (tagged "Fast"). */
  fast: boolean;
  /** The provider's own default. */
  default: boolean;
}

export interface ModelGroup {
  provider: string;
  label: string;
  icon: string;
  state: "connected" | "needs-reconnect";
  /** The coding app that must run these models (only its own sign-in is saved); null: the general agent. */
  harness: string | null;
  models: ModelChoice[];
}

/** The models of the connected providers (`connectors.models`). */
export interface ModelsView {
  groups: ModelGroup[];
  /** What a new prompt starts with. */
  default: { provider: string; model: string } | null;
  connectLabel: string;
  empty: string | null;
}

export const modelsView = (core: CoreClient, input: ConnectorsInput): ModelsView | null =>
  core.tryCall<ModelsView>("connectors.models", { input: input as unknown as Json }) ?? null;

export const appDetail = (core: CoreClient, input: ConnectorsInput, agent: string): AppDetailView | null =>
  core.tryCall<AppDetailView | null>("connectors.appDetail", { input: input as unknown as Json, agent }) ?? null;

/** The card for an app that isn't connected; `provider` is the one a run said refused it, when it did. */
export const setupCard = (core: CoreClient, input: ConnectorsInput, agent: string, name: string, provider: string | null = null): SetupCardView | null =>
  core.tryCall<SetupCardView>("connectors.setupCard", { input: input as unknown as Json, agent, name, provider }) ?? null;

/* ---- Summoning agents in a Space ---------------------------------------------------- */

export interface SummonMember {
  handle: string;
  harness: string;
  label: string;
}

export interface SummonInput {
  members: SummonMember[];
  people: string[];
  canAdd: boolean;
  connectors: ConnectorsInput;
}

export interface SummonCandidate {
  handle: string;
  label: string;
  harness: string;
  member: boolean;
  detail: string;
}

export interface CantWake {
  reason: "not-connected" | "cant-add" | "failed";
  handle: string;
  harness: string;
  name: string;
  app: string;
  text: string;
  detail: string;
  connectLabel: string | null;
  connectingLabel: string;
  openLabel: string | null;
  retryLabel: string;
  retriedText: string;
  /** Only you see it: never posted to the thread. */
  onlyYou: boolean;
  onlyYouLabel: string;
  dismissLabel: string;
}

/** A line only this device shows in a thread (never posted). */
export interface LocalNote {
  text: string;
  onlyYou: boolean;
  onlyYouLabel: string;
  dismissLabel: string | null;
}

export const queuedNote = (core: CoreClient, creating: boolean): LocalNote | null => core.tryCall<LocalNote>("summon.queuedNote", { creating }) ?? null;

export interface SummonStep {
  handle: string;
  harness: string;
  action: "wake" | "add" | "blocked";
  cantWake: CantWake | null;
}

export interface FirstMessage {
  text: string;
  handle: string | null;
  harness: string;
}

export const summonCandidates = (core: CoreClient, input: SummonInput): SummonCandidate[] | null =>
  core.tryCall<SummonCandidate[]>("summon.candidates", { input: input as unknown as Json }) ?? null;

export const summonPlan = (core: CoreClient, input: SummonInput, text: string): SummonStep[] | null =>
  core.tryCall<{ steps: SummonStep[] }>("summon.plan", { input: input as unknown as Json, text })?.steps ?? null;

export const summonFailed = (core: CoreClient, input: SummonInput, harness: string, handle: string, error: string): CantWake | null =>
  core.tryCall<CantWake | null>("summon.failed", { input: input as unknown as Json, harness, handle, error }) ?? null;

export const firstMessage = (core: CoreClient, input: SummonInput, agent: string, prompt: string): FirstMessage | null =>
  core.tryCall<FirstMessage>("summon.firstMessage", { input: input as unknown as Json, agent, prompt }) ?? null;

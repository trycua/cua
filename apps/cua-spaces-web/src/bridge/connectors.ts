// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Connectors (redesign) and summoning agents in a Space: the
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

/* ---- Connectors ---------------------------------------------------------------- */

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
  keys: AgentKeysInput;
  shellKeys: string[];
  auto: boolean;
  off: string[];
  busy: { id: string; step: "connecting" | "signing-in" | "signing-out" }[];
  failures: { id: string; line: string }[];
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

export interface ConnectorProviderRow {
  id: string;
  label: string;
  state: "connected" | "not-connected" | "connecting" | "failed";
  status: string;
  detail: string;
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
  providers: ConnectorProviderRow[];
  others: AgentKeyRowView[];
  addOtherLabel: string;
  otherHelp: string;
  notice: string | null;
  canEdit: boolean;
  addedLabel: string;
}

export interface ConnectPlan {
  tools: boolean;
  appToken: boolean;
  device: boolean;
  provider: string | null;
  tokenProvider: string | null;
}

export interface ProviderWay {
  id: "app-token" | "app-device" | "key" | "shell";
  label: string;
  help: string;
  button: string;
  app: string | null;
  env: string | null;
}

export interface ProviderSheetView {
  provider: string;
  title: string;
  lede: string;
  ways: ProviderWay[];
  cancelLabel: string;
  keyLabel: string;
  keyPlaceholder: string;
  keyHelp: string;
  saveLabel: string;
  keyProvider: "anthropic" | "openai" | "other";
  keyEnv: string | null;
  /** "Or choose another provider", when opened for an app that signs in with more than one. */
  othersLabel: string | null;
  others: { provider: string; label: string }[];
}

export interface ConnectorToggle {
  id: "skills" | "mcp" | "auto";
  label: string;
  help: string;
  on: boolean;
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
    keys,
    shellKeys: extra.shellKeys ?? [],
    auto: extra.auto ?? false,
    off: extra.off ?? [],
    busy: extra.busy ?? [],
    failures: extra.failures ?? [],
  };
}

export const connectorsView = (core: CoreClient, input: ConnectorsInput, os: HostOs = hostOs()): ConnectorsView | null =>
  core.tryCall<ConnectorsView>("connectors.view", { input: input as unknown as Json, hostOs: os }) ?? null;

export const connectPlan = (core: CoreClient, input: ConnectorsInput, agent: string): ConnectPlan | null =>
  core.tryCall<ConnectPlan>("connectors.connectPlan", { input: input as unknown as Json, agent }) ?? null;

export const providerSheet = (core: CoreClient, input: ConnectorsInput, provider: string, os: HostOs = hostOs(), forApp: string | null = null): ProviderSheetView | null =>
  core.tryCall<ProviderSheetView | null>("connectors.providerSheet", { input: input as unknown as Json, provider, hostOs: os, forApp }) ?? null;

export const appDetail = (core: CoreClient, input: ConnectorsInput, agent: string): AppDetailView | null =>
  core.tryCall<AppDetailView | null>("connectors.appDetail", { input: input as unknown as Json, agent }) ?? null;

export const setupCard = (core: CoreClient, input: ConnectorsInput, agent: string, name: string): SetupCardView | null =>
  core.tryCall<SetupCardView>("connectors.setupCard", { input: input as unknown as Json, agent, name }) ?? null;

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

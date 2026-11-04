// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Preparing the ask-first Space (the Linux runtime, the agent image, the home
 * Space, the coding agent): types mirroring
 * `cua-spaces-app-core/src/prepare.rs` (the state machine, its page and its
 * numbers).
 */

export type PrepareStepId = "runtime" | "image" | "home" | "agent";
export type PrepareStepState = "pending" | "running" | "waiting" | "failed" | "done" | "skipped";
export type PrepareMessageState = "queued" | "delivering" | "delivered";

export interface PrepareStepRecord {
  state: PrepareStepState;
  done: number;
  total: number;
  perSecond: number | null;
  startedMs: number | null;
  finishedMs: number | null;
}

export interface PrepareMessage {
  id: string;
  text: string;
  agent: string;
  model: string | null;
  repo: string | null;
  state: PrepareMessageState;
  attempts: number;
  sentMs: number;
  claimedMs: number | null;
  deliveredMs: number | null;
  firstTokenMs: number | null;
}

/** The record the host keeps between launches (`schema` 1). */
export interface PrepareState {
  schema: number;
  wanted: boolean;
  startedMs: number | null;
  onboardingDoneMs: number | null;
  runtime: PrepareStepRecord;
  image: PrepareStepRecord;
  home: PrepareStepRecord;
  agent: PrepareStepRecord;
  harness: string;
  homeSpaceId: string | null;
  pendingId: string | null;
  attempt: number;
  failures: number;
  retryAtMs: number | null;
  trouble: { network: boolean; message: string } | null;
  online: boolean;
  readyMs: number | null;
  /** When a person first wrote into the home; until then no list shows it. */
  askedMs: number | null;
  messages: PrepareMessage[];
}

export type PrepareAction =
  | { type: "start"; nowMs: number; harness?: string | null; pendingId?: string | null }
  | { type: "onboardingDone"; nowMs: number }
  | { type: "phase"; nowMs: number; phase: string; detail?: string; done?: number | null; total?: number | null; perSecond?: number | null }
  | { type: "runtimeNotNeeded"; nowMs: number }
  | { type: "homeReady"; nowMs: number; spaceId: string }
  | { type: "agentReady"; nowMs: number }
  | { type: "failed"; nowMs: number; message: string; network?: boolean }
  | { type: "online"; nowMs: number; online: boolean }
  | { type: "tick"; nowMs: number }
  | { type: "retry"; nowMs: number }
  | { type: "resume"; nowMs: number }
  | { type: "homeGone"; nowMs: number }
  | { type: "send"; nowMs: number; id: string; text: string; agent?: string; model?: string | null; repo?: string | null }
  | { type: "delivered"; nowMs: number; id: string }
  | { type: "deliveryFailed"; nowMs: number; id: string }
  | { type: "threadHas"; nowMs: number; id: string; found: boolean }
  | { type: "firstToken"; nowMs: number; id: string }
  | { type: "asked"; nowMs: number };

export type PrepareEffect =
  | { type: "create" }
  | { type: "attachAgent" }
  | { type: "deliver"; id: string }
  | { type: "verifyDelivery"; id: string }
  | { type: "wakeAt"; atMs: number };

export interface PrepareReduced {
  state: PrepareState;
  effects: PrepareEffect[];
}

export interface PrepareStepView {
  id: PrepareStepId;
  label: string;
  state: PrepareStepState;
  detail: string | null;
  /** 0 to 1: the one thin bar of the block, under the current step only. */
  fraction: number | null;
}

export interface PrepareQueuedView {
  id: string;
  text: string;
  note: string;
}

export interface PrepareView {
  phase: "idle" | "preparing" | "waiting" | "failed" | "ready";
  title: string;
  /** Only when the steps cannot say it (connection lost, stopped, ready); null while preparing. */
  line: string | null;
  steps: PrepareStepView[];
  queued: PrepareQueuedView[];
  composerHint: string | null;
  action: { id: string; label: string } | null;
}

export interface PrepareMetrics {
  runtimeMs: number | null;
  imageMs: number | null;
  homeMs: number | null;
  agentMs: number | null;
  prepareMs: number | null;
  onboardingToReadyMs: number | null;
  messages: { id: string; sendToDeliveredMs: number | null; sendToFirstTokenMs: number | null }[];
}

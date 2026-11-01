// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The fault points of the crash campaign: where a worker can be killed (or
// frozen), and how long after the marker to do it (ms). Each name is a
// `MARK <name>` line printed by agent.mjs: the durable layer's own trace
// points (src/durable.ts) plus the wrapper points of agent.mjs (around the
// Space's commands and snapshots). test/crash-drift.test.mjs keeps this list,
// those sources and the guide in step.

/** Checkpoint mode: a checkpoint of the Space after every call. */
export const CHECKPOINT_FAULTS = [
  ["started", [0, 30]], // intent and start mark committed, before the command starts
  ["exec-spawn", [0, 5]], // before or while the command starts
  ["exec-running", [20, 250]], // mid-command
  ["exec-exit", [0, 20]], // command exited, outcome not yet committed
  ["outcome", [0, 10]], // outcome committed, before the checkpoint
  ["checkpoint-begin", [0, 40]], // during the checkpoint
  ["checkpointed", [0, 10]], // checkpoint taken, not yet recorded
  ["recorded", [0, 10]], // recorded, result entry not yet committed
  ["restore-begin", [0, 30]], // during a recovery restore (only after an earlier fault)
  ["call", [0, 5]], // a call (or a recovery) about to decide
]

/** Transactional mode: the call's diff is committed with its outcome; no checkpoint. */
export const TRANSACTIONAL_FAULTS = [
  ["call", [0, 5]], // a call (or a recovery) about to decide
  ["started", [0, 30]], // before-image and intent committed, before the command starts
  ["exec-spawn", [0, 5]],
  ["exec-running", [20, 250]],
  ["exec-exit", [0, 20]], // command exited, nothing committed
  ["executed", [0, 20]], // the tool returned, the call's diff not yet computed
  ["diffed", [0, 10]], // the call's diff is computed, not yet committed
  ["outcome", [0, 10]], // outcome and diff committed
  ["undo-committed", [0, 10]], // recovery: the decision to undo is committed
  ["undoing", [0, 40]], // recovery: the undo is in progress
  ["reverted", [0, 10]], // recovery: the workspace is back at the image
]

/** Auto mode starts transactional and may fall back to checkpoints: both sets. */
export const AUTO_FAULTS = [...TRANSACTIONAL_FAULTS, ...CHECKPOINT_FAULTS.filter(([p]) => !TRANSACTIONAL_FAULTS.some(([q]) => q === p))]

/** Marks that exist but are not fault points (progress only). */
export const PROGRESS_MARKS = ["open", "checkpoint-end", "restore-end"]

/** Points that only occur while recovering from an earlier fault: the runner aims at them after a kill. */
export const RECOVERY_POINTS = ["restore-begin", "undo-committed", "undoing", "reverted"]

/** Points every call passes through (not the ones only exec'd steps or recoveries reach): a fault there always lands. */
export const EVERY_CALL = ["call", "started", "executed", "diffed", "outcome", "checkpoint-begin", "checkpointed", "recorded"]

/**
 * ACP mode: where the harness-driving runner is attacked. Not markers of agent.mjs: each is a kind of kill
 * acp.mjs injects while a tool call is in flight (delay in ms after the call is seen in flight), or a fault
 * point the preload acp-fault.mjs implements around the unchanged runner.
 */
export const ACP_FAULTS = [
  ["runner", [300, 1800]], // SIGKILL of the runner (the container goes with it)
  ["harness", [300, 1800]], // SIGKILL of the harness process; the runner restarts it in process
  ["container", [300, 1800]], // the whole container is killed
  ["coordinator", [300, 1800]], // the next epoch starts while the old driver still runs
  ["before_intent", [0, 0]], // preload: permission request in, nothing journaled
  ["after_intent", [0, 0]], // preload: intent fsynced, no answer yet
]

export const FAULTS_BY_MODE = {
  checkpoint: CHECKPOINT_FAULTS,
  transactional: TRANSACTIONAL_FAULTS,
  auto: AUTO_FAULTS,
  // A stale worker is frozen at any point a call can be at.
  stale: TRANSACTIONAL_FAULTS.filter(([p]) => !["undo-committed", "undoing", "reverted"].includes(p)),
  redrive: AUTO_FAULTS,
  acp: ACP_FAULTS,
}

/** The modes `CRASH_MODE` accepts. */
export const MODES = ["checkpoint", "transactional", "auto", "stale", "redrive", "acp"]

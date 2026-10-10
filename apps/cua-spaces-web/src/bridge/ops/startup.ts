// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The startup gate: while the native app is still starting (reading the
 * saved sign-in from the macOS Keychain, or starting the Cua daemon), the
 * page shows a full-window startup screen instead of the app.
 *
 * The host owns the state and the words; the page shows them and sends the
 * buttons back (`startup.act`). The host pushes `startup.changed` with the
 * new state.
 *
 * - SwiftUI: `startup.get` and `startup.act`. An older build answers
 *   `unimplemented`; that, and any other failure, reads as ready, so the
 *   page never blocks on this.
 * - Tauri: no startup gate; answered as ready in the page, with no command.
 * - Electron: the same methods: its launch (the keychain check on macOS,
 *   the daemon, the SDK) is the SwiftUI app's.
 */

import type { OpCoverage } from "../coverage";

export type StartupPhase = "starting" | "needsKeychain" | "waitingForKeychain" | "keychainDenied" | "startFailed" | "ready";
export type StartupAction = "allowAccess" | "tryAgain" | "signInAgain";

export interface StartupState {
  phase: StartupPhase;
  /** Waiting longer than expected (the prompt is still up, or the daemon is slow). */
  slow: boolean;
  /** The host's words, shown as is. */
  title: string;
  body: string;
  /** The buttons to show, in order (the first is the primary one). */
  actions: StartupAction[];
}

export const READY_STARTUP: StartupState = { phase: "ready", slow: false, title: "", body: "", actions: [] };

export interface StartupOps {
  "startup.get": { args: Record<string, never>; result: StartupState };
  "startup.act": { args: { action: StartupAction }; result: StartupState };
}

export type StartupHostEvent = { type: "startup.changed"; state: StartupState };
export const STARTUP_EVENTS = ["startup.changed"] as const;

export const STARTUP_OPERATIONS = ["startup.get", "startup.act"] as const satisfies readonly (keyof StartupOps)[];

const PHASES: readonly StartupPhase[] = ["starting", "needsKeychain", "waitingForKeychain", "keychainDenied", "startFailed", "ready"];
const ACTIONS: readonly StartupAction[] = ["allowAccess", "tryAgain", "signInAgain"];

/** A host's answer as a state; anything malformed reads as ready. */
export function startupFromHost(raw: unknown): StartupState {
  const s = raw as Partial<StartupState> | null | undefined;
  if (!s || typeof s !== "object" || !PHASES.includes(s.phase as StartupPhase)) return READY_STARTUP;
  if (s.phase === "ready") return READY_STARTUP;
  return {
    phase: s.phase as StartupPhase,
    slow: s.slow === true,
    title: typeof s.title === "string" ? s.title : "",
    body: typeof s.body === "string" ? s.body : "",
    actions: Array.isArray(s.actions) ? s.actions.filter((a): a is StartupAction => ACTIONS.includes(a as StartupAction)) : [],
  };
}

/* ---- Hosts ------------------------------------------------------------------ */

type WebkitRequest = <T>(method: "startup.get" | "startup.act", args?: Record<string, unknown>, wait?: number | null) => Promise<T>;

/** SwiftUI: both methods go to the host. Older builds (`unimplemented`) and
 * failures read as ready. */
export function webkitStartupOps(request: WebkitRequest) {
  const get = () => request<unknown>("startup.get", {}, 5_000).then(startupFromHost, () => READY_STARTUP);
  return {
    "startup.get": get,
    // A failed action: show what the host says now.
    "startup.act": ({ action }: StartupOps["startup.act"]["args"]) =>
      request<unknown>("startup.act", { action }).then(startupFromHost, get),
  };
}

/** Tauri has no startup gate. */
export function tauriStartupOps() {
  return {
    "startup.get": async () => READY_STARTUP,
    "startup.act": async () => READY_STARTUP,
  };
}

export const STARTUP_COVERAGE = {
  "startup.get": {
    webkit: { methods: ["startup.get"] },
    // No command: Tauri has no startup gate, the page answers ready.
    tauri: [],
  },
  "startup.act": {
    webkit: { methods: ["startup.act", "startup.get"] },
    tauri: [],
  },
} as const satisfies Record<keyof StartupOps, OpCoverage>;

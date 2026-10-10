// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's startup gate: ready, or with `?demo=keychain` the macOS
 * Keychain prompt the SwiftUI app walks through. Allow access waits for the
 * prompt, then is ready; Try again waits again; Sign in again is ready. The
 * words mirror the native app's.
 */

import { READY_STARTUP, type StartupAction, type StartupPhase, type StartupState } from "../../ops/startup";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoStartupState {
  startup: StartupState;
}

/** How long the demo "prompt" stays up before it is answered (ms). */
export const DEMO_KEYCHAIN_WAIT_MS = 1500;

const KEYCHAIN_WAIT_BODY =
  "macOS is asking for your login password so Cua Spaces can read your sign-in. Look for the prompt (it can be behind other windows). Enter your password and choose Always Allow.";

/** The state for a phase, in the native app's words. */
export function demoStartup(phase: StartupPhase, slow = false): StartupState {
  switch (phase) {
    case "ready":
      return READY_STARTUP;
    case "needsKeychain":
      return {
        phase,
        slow,
        title: "Allow Keychain access",
        body: "Cua Spaces keeps your sign-in in the macOS Keychain. This version needs your permission to read it. Click Allow access, then enter your Mac login password and choose Always Allow.",
        actions: ["allowAccess", "signInAgain"],
      };
    case "waitingForKeychain":
      return slow
        ? { phase, slow, title: "Still waiting for Keychain access", body: KEYCHAIN_WAIT_BODY, actions: ["tryAgain", "signInAgain"] }
        : { phase, slow, title: "Waiting for Keychain access…", body: KEYCHAIN_WAIT_BODY, actions: [] };
    case "keychainDenied":
      return {
        phase,
        slow,
        title: "Keychain access was not allowed",
        body: "Cua Spaces can't read your sign-in without it. Try again and choose Always Allow, or sign in again.",
        actions: ["tryAgain", "signInAgain"],
      };
    case "startFailed":
      return {
        phase,
        slow,
        title: "Cua's background service didn't start",
        body: "Cua Spaces couldn't start its background service. Click Try again to start it again.",
        actions: ["tryAgain"],
      };
    case "starting":
      return slow
        ? { phase, slow, title: "Still starting Cua…", body: "This can take a minute after an update.", actions: [] }
        : { phase, slow, title: "Starting Cua…", body: "", actions: [] };
  }
}

export const demoStartupState = (keychain: boolean): DemoStartupState => ({
  startup: keychain ? demoStartup("needsKeychain") : READY_STARTUP,
});

export function demoStartupHandlers({ state, wait, emit }: DemoContext): DemoHandlers<"startup.get" | "startup.act"> {
  let attempt = 0;
  const set = (next: StartupState) => {
    state.startup = next;
    emit({ type: "startup.changed", state: next });
    return next;
  };
  const waitForPrompt = () => {
    const mine = ++attempt;
    void wait(DEMO_KEYCHAIN_WAIT_MS).then(() => {
      if (mine === attempt && state.startup.phase === "waitingForKeychain") set(READY_STARTUP);
    });
    return set(demoStartup("waitingForKeychain"));
  };
  return {
    "startup.get": () => state.startup,
    "startup.act": ({ action }: { action: StartupAction }) => {
      if (state.startup.phase === "ready") return state.startup;
      switch (action) {
        case "allowAccess":
        case "tryAgain":
          return waitForPrompt();
        case "signInAgain":
          attempt++;
          return set(READY_STARTUP);
        default:
          return state.startup;
      }
    },
  };
}

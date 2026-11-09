// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Reset & Show": the page forgets exactly the welcome's state (the app
// core's reset plan) and nothing else, and doing it twice is the same as once.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { readFirstRun, resetFirstRunCache, TEAMMATES_KEY, UPSELL_DISMISSED_KEY, writeFirstRun } from "../first-run";
import { readSavedOnboarding, resetSavedOnboardingCache, writeSavedOnboarding } from "../onboarding";
import { FALLBACK_RESET_PLAN, forgetWelcome, onboardingResetPlan } from "../onboarding-reset";
import { STATE_KEY } from "@/lib/connected-apps";
import { connectedAppsState, resetConnectedAppsState, setConnectedAppsState } from "@/hooks/use-connected-apps";
import { noCore, testCore, wasmBuilt } from "./testCore";

/** The golden of `onboarding_reset::plan` (cua-spaces-app-core/contracts). */
const golden = JSON.parse(
  readFileSync(join(import.meta.dirname, "../../../../../libs/cua/crates/cua-spaces-app-core/contracts/onboarding-reset.json"), "utf8"),
) as { webKeys: string[]; hostFiles: string[]; firstStep: string };

/** Everything else the app keeps in the page: none of it is the welcome's. */
const OTHER: Record<string, string> = {
  "cua-spaces:theme": "dark",
  "cua-spaces:ui": '{"state":{"sidebar":true}}',
  "cua-spaces:checklist": '{"done":["create_space"],"dismissed":true,"shown":true,"skipped":[]}',
  "cua-spaces:notifications-seen": "12",
  "cua-spaces:layout:local:a": "thread",
  "cua-spaces:call-split": "0.7",
  "cua.somethingElse": "1",
};

function fillWelcome() {
  writeSavedOnboarding({
    state: { step: "agents", identity: "ada@example.com", installerMode: null, mode: "host", menuBar: true, telemetry: null } as never,
    skipped: true,
  });
  writeFirstRun({ step: "setup", picked: ["claude-code"], setupStartedMs: 5, usedAtMs: 9, extraNotNow: true, finishedLater: true, todosClosed: true });
  localStorage.setItem(UPSELL_DISMISSED_KEY, "1");
  localStorage.setItem(TEAMMATES_KEY, "1");
  setConnectedAppsState({ known: ["claude-code", "codex"], off: ["codex"], auto: true });
}

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  resetFirstRunCache();
  resetConnectedAppsState();
});
afterEach(() => localStorage.clear());

describe("the welcome's reset plan", () => {
  it("is the app core's list, with or without the core", async () => {
    expect(FALLBACK_RESET_PLAN).toEqual(golden);
    expect(onboardingResetPlan(noCore)).toEqual(golden);
    if (wasmBuilt) expect(onboardingResetPlan(await testCore())).toEqual(golden);
  });

  it("names the keys the page really writes", () => {
    fillWelcome();
    const written = Object.keys({ ...localStorage }).sort();
    expect(written).toEqual([...golden.webKeys].sort());
    expect(STATE_KEY).toBe("cua.connectedApps");
  });
});

describe("forgetWelcome", () => {
  it("clears exactly the welcome's keys and starts the first run at its first step", () => {
    fillWelcome();
    for (const [k, v] of Object.entries(OTHER)) localStorage.setItem(k, v);
    expect(readFirstRun().step).toBe("setup");

    forgetWelcome(noCore);

    for (const k of golden.webKeys) expect(localStorage.getItem(k), k).toBeNull();
    for (const [k, v] of Object.entries(OTHER)) expect(localStorage.getItem(k), k).toBe(v);
    // The copies in memory are fresh too, so a page that is open starts over.
    expect(readFirstRun().step).toBe(golden.firstStep);
    expect(readFirstRun().picked).toBeNull();
    expect(readFirstRun().finishedLater).toBe(false);
    expect(readSavedOnboarding()).toEqual({ state: null, skipped: false });
    expect(connectedAppsState()).toEqual({ known: null, off: [], auto: null });
  });

  it("is safe to run twice, and on a page with nothing saved", () => {
    forgetWelcome(noCore);
    fillWelcome();
    for (const [k, v] of Object.entries(OTHER)) localStorage.setItem(k, v);
    forgetWelcome(noCore);
    const once = { ...localStorage };
    forgetWelcome(noCore);
    expect({ ...localStorage }).toEqual(once);
    expect(once).toEqual(OTHER);
  });
});

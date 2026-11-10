// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The last onboarding page of an ask-first run ends on Home (the prompt box and the suggestions), not inside a
// Space: the nothing is made in the background: a Space exists only when the person chooses one.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetFirstRunCache } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetAskTasks } from "@/lib/ask-first";
import { rememberAgent, resetPickedAgent } from "@/lib/ask-first-model";
import { getDriver, resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  resetFirstRunCache();
  resetConnectedAppsState();
  resetPrepare();
  resetAskTasks();
  // The agent the first run prepares a home for is the one the person last used.
  resetPickedAgent();
  rememberAgent({ harness: "claude-code" });
});
afterEach(() => {
  cleanup();
  resetPrepare();
});

const click = (sel: string) => fireEvent.click(document.querySelector<HTMLElement>(sel)!);
const heading = () => document.querySelector("[data-onboarding-heading]")?.textContent ?? "";

async function throughOnboarding(hold?: Promise<void>, calls: { op: string }[] = []) {
  const { router } = await mountApp("/onboarding", {
    failsCreates: false,
    listsGhost: false,
    hold,
    intercept: (op) => (calls.push({ op }), undefined),
    demo: { signedIn: false, onboarded: false, noSpaces: true, keys: "several" },
  });
  // The new first run (CUA-3318): Skip for now, the agent found here, Ready, then Home.
  await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
  click("[data-onboarding-later]");
  // On a Mac, Cua Spaces' permissions come next (CUA-3550); a person can go on without them.
  await waitFor(() => expect(document.querySelector("[data-new-run=permissions], [data-new-run=agent]")).not.toBeNull(), { timeout: 15_000 });
  if (document.querySelector("[data-new-run=permissions]")) click("[data-onboarding-primary]");
  await waitFor(() => expect(document.querySelector("[data-new-run=agent]:not([data-state=looking])")).not.toBeNull(), { timeout: 15_000 });
  // Every agent found is ticked: Continue connects them, with nothing to pick first.
  click("[data-onboarding-primary]");
  await waitFor(() => expect(document.querySelector("[data-new-run=ready]")).not.toBeNull(), { timeout: 15_000 });
  // Nothing is made while the first run shows.
  expect(getDriver()?.state?.wanted ?? false).toBe(false);
  // Ready is the last screen: Done ends setup (the rest is asked for on Home after the first job, CUA-3150).
  click("[data-onboarding-primary]");
  return router;
}

describe.skipIf(!wasmBuilt)("where the first run ends", () => {
  it("lands on Home with your computers and Run something, and makes no Space the person did not choose", { timeout: 60_000 }, async () => {
    const calls: { op: string }[] = [];
    const router = await throughOnboarding(undefined, calls);
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"));
    // Your computers as one pool, with Run something and Add a computer, not a Space's page.
    await waitFor(() => expect(document.querySelector("[data-pool-grid]")).not.toBeNull());
    expect(document.querySelector("[data-pool-run]")).not.toBeNull();
    expect(document.querySelector("[data-pool-add-tile]")).not.toBeNull();
    // Nothing is warming in the background: no Space is created and nothing is being prepared.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1_500));
    });
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
    expect(document.querySelector("[data-prepare-line]")).toBeNull();
    expect(document.querySelector("[data-home-spaces]")?.textContent ?? "").not.toContain("home-claude-code");
    expect(document.body.textContent).not.toContain("home-claude-code");
    // No job has run yet, so Home doesn't ask for the rest of setup.
    expect(document.querySelector("[data-bring-rest]")).toBeNull();
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The last onboarding page of an ask-first run ends on Home (the prompt box and the suggestions), not inside a
// Space: the first agent starts getting ready in the background once the first run is over, with no line about it.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetFirstRunCache } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetAskTasks } from "@/lib/ask-first";
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
  setFlagOverride("spaces_ask_first_home", true);
  setFlagOverride("spaces_new_onboarding", true);
});
afterEach(() => {
  cleanup();
  setFlagOverride("spaces_ask_first_home", null);
  setFlagOverride("spaces_new_onboarding", null);
  resetPrepare();
});

const click = (sel: string) => fireEvent.click(document.querySelector<HTMLElement>(sel)!);
const heading = () => document.querySelector("[data-onboarding-heading]")?.textContent ?? "";

async function throughOnboarding(hold?: Promise<void>) {
  const { router } = await mountApp("/onboarding", { failsCreates: false, listsGhost: false, hold, demo: { signedIn: false, onboarded: false, noSpaces: true, keys: "several" } });
  // The new first run (CUA-3318): Skip for now, this computer, the agent found here, Ready, next steps.
  await waitFor(() => expect(heading()).toBe("Bring all your computers under one account"));
  click("[data-onboarding-later]");
  await waitFor(() => expect(heading()).toBe("Pick where your agents work"));
  click("[data-onboarding-primary]");
  await waitFor(() => expect(document.querySelector("[data-new-run=agent]:not([data-state=looking])")).not.toBeNull(), { timeout: 15_000 });
  const several = document.querySelector("[data-new-run=agent][data-state=several]");
  if (several) click("[data-new-run-agent=claude-code]");
  click("[data-onboarding-primary]");
  await waitFor(() => expect(document.querySelector("[data-new-run=ready]")).not.toBeNull(), { timeout: 15_000 });
  click("[data-onboarding-primary]");
  await waitFor(() => expect(heading()).toBe("Now bring the rest in"));
  // Nothing is made while the first run shows.
  expect(getDriver()?.state?.wanted ?? false).toBe(false);
  click("[data-onboarding-primary]");
  return router;
}

// The welcome's example jobs are still there (Home's own suggestions).
const expectCards = () => {
  expect(document.querySelectorAll("[data-home-card]").length).toBeGreaterThanOrEqual(2);
};

describe.skipIf(!wasmBuilt)("where the first run ends", () => {
  it("lands on Home with the prompt box and the suggestions, and says nothing about the home warming", { timeout: 60_000 }, async () => {
    let release!: () => void;
    const router = await throughOnboarding(new Promise<void>((r) => (release = r)));
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"));
    // The prompt box and the suggestions are there, not a Space's page.
    expect(await screen.findByLabelText("What do you want to do?")).not.toBeNull();
    expectCards();
    expect(document.querySelector("[data-ask-first-home]")).not.toBeNull();
    // Warming is silent: no line, no steps, and the home is not in "Your Spaces".
    await waitFor(() => expect(getDriver()?.state?.pendingId).toBeTruthy());
    await act(async () => {
      await new Promise((r) => setTimeout(r, 600));
    });
    expect(document.querySelector("[data-prepare-line]")).toBeNull();
    expect(document.querySelector("[data-prepare-steps]")).toBeNull();
    expect(document.body.textContent).not.toContain("Getting your agent ready");
    expect(document.querySelector("[data-home-spaces]")?.textContent ?? "").not.toContain("home-claude-code");
    // A prompt sent now is kept and goes once the agent is ready (the Space's thread says so).
    fireEvent.change(screen.getByLabelText("What do you want to do?"), { target: { value: "fix the build" } });
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await waitFor(() => expect(getDriver()?.state?.messages.map((m) => m.text)).toEqual(["fix the build"]));
    release();
  });

  it("when everything is ready nothing is shown and the home is not in Your Spaces", { timeout: 60_000 }, async () => {
    const router = await throughOnboarding();
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"));
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30_000 });
    await waitFor(() => expect(document.querySelector("[data-prepare-line]")).toBeNull());
    expect(screen.getByLabelText("What do you want to do?")).not.toBeNull();
    expect(document.querySelector("[data-home-spaces]")?.textContent ?? "").not.toContain("home-claude-code");
    expect(document.body.textContent).not.toContain("home-claude-code");
    // The home that was made meanwhile does not take the suggestions away: nothing was asked yet.
    expectCards();
  });
});

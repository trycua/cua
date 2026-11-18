// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, About: "Show the welcome again" has two actions. Show opens the
// welcome as it was; "Reset & Show" asks first, and only on confirming does
// it clear what the welcome kept and start it at the first step.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { readFirstRun, resetFirstRunCache, TEAMMATES_KEY, writeFirstRun } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { resetConnectedAppsState, setConnectedAppsState } from "@/hooks/use-connected-apps";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

stubBrowserApis();
afterEach(() => {
  cleanup();
  localStorage.clear();
});
beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  resetFirstRunCache();
  resetConnectedAppsState();
});

const calls: string[] = [];
const host: TestHost = {
  failsCreates: false,
  listsGhost: false,
  intercept: (op) => {
    if (op === "session.resetOnboarding") calls.push(op);
    return undefined;
  },
};

const resetButton = () => document.querySelector<HTMLButtonElement>("[data-about-welcome-reset]")!;
const dialog = () => document.querySelector<HTMLElement>('[role="alertdialog"]');
const kept = () => ({ ...localStorage });

function seed() {
  writeFirstRun({ step: "setup", picked: ["claude-code"], finishedLater: true });
  localStorage.setItem(TEAMMATES_KEY, "1");
  localStorage.setItem("cua-spaces:theme", "dark");
  setConnectedAppsState({ known: ["codex"], off: [], auto: true });
  calls.length = 0;
}

async function open() {
  const app = await mountApp("/settings/about", { ...host, demo: { onboarded: true } });
  await waitFor(() => expect(resetButton()).not.toBeNull());
  return app;
}

describe.skipIf(!wasmBuilt)("About, Reset & Show", () => {
  it("keeps Show and adds Reset & Show beside it", async () => {
    await open();
    expect(document.querySelector("[data-about-welcome]")!.textContent).toBe("Show");
    expect(resetButton().textContent).toBe("Reset & Show");
  });

  it("asks first, in words that say what stays, and starts on Cancel", async () => {
    seed();
    await open();
    fireEvent.click(resetButton());
    await waitFor(() => expect(dialog()).not.toBeNull());
    expect(dialog()!.textContent).toContain("Reset the welcome?");
    expect(dialog()!.textContent).toContain("Your Spaces, computers, Keyvault and sign-in are not affected.");
    expect(dialog()!.textContent).not.toContain("·");
    expect(dialog()!.textContent).not.toMatch(/machine/i);
    const labels = [...dialog()!.querySelectorAll("button")].map((b) => b.textContent);
    expect(labels).toEqual(["Cancel", "Reset & Show"]);
    await waitFor(() => expect(document.activeElement?.textContent).toBe("Cancel"));
    // Nothing happened yet.
    expect(calls).toEqual([]);
    expect(readFirstRun().step).toBe("setup");
  });

  it("Cancel, Escape and a click outside do nothing", async () => {
    seed();
    const { router } = await open();
    const before = kept();
    for (const how of ["cancel", "escape", "outside"] as const) {
      fireEvent.click(resetButton());
      await waitFor(() => expect(dialog()).not.toBeNull());
      if (how === "cancel") fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      else if (how === "escape") fireEvent.keyDown(dialog()!, { key: "Escape" });
      else fireEvent.click(document.querySelector<HTMLElement>("[data-window-dim]")!);
      await waitFor(() => expect(dialog()).toBeNull());
    }
    expect(calls).toEqual([]);
    expect(kept()).toEqual(before);
    expect(router.state.location.pathname).toBe("/settings/about");
  });

  it("confirming resets the welcome once and shows it at the first step", async () => {
    seed();
    const { router } = await open();
    fireEvent.click(resetButton());
    await waitFor(() => expect(dialog()).not.toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "Reset & Show" }));

    await waitFor(() => expect(router.state.location.pathname).toBe("/onboarding"));
    expect(calls).toEqual(["session.resetOnboarding"]);
    // Only the welcome's keys went; the theme stays.
    expect(localStorage.getItem(TEAMMATES_KEY)).toBeNull();
    expect(localStorage.getItem("cua-spaces:theme")).toBe("dark");
    expect(readFirstRun().step).toBe("welcome");
    // The welcome's first page is on screen.
    await waitFor(() => expect(document.querySelector("[data-new-first-run]")?.getAttribute("data-step")).toBe("welcome"));
    expect(document.body.textContent).toContain("Combine all your computers into one supercomputer.");
    await act(async () => {});
    expect(readFirstRun().step).toBe("welcome");
  });

  it("Show, as before, asks nothing and resets nothing", async () => {
    seed();
    const { router } = await open();
    const before = kept();
    fireEvent.click(document.querySelector("[data-about-welcome]")!);
    await waitFor(() => expect(router.state.location.pathname).toBe("/onboarding"));
    expect(dialog()).toBeNull();
    expect(calls).toEqual([]);
    expect(localStorage.getItem(TEAMMATES_KEY)).toBe(before[TEAMMATES_KEY]);
  });
});

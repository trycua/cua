// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The `warm-pools` experiment (Settings, Advanced, Labs, "Warm start") keeps the home that warms in the background
// for Home's prompt box, which claims the daemon's warm Space. It is invisible infrastructure until a person writes
// into it: not on the Spaces list, in the sidebar or the palette, and not among the agents. Its own page and
// Home's preparing line still show it. The rule is the core's (`prepare.hidden`).
// With the experiment off the app makes no Space a person did not choose (CUA-3622; `prepare-launch.test.tsx`).

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetFirstRunCache } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetAskTasks } from "@/lib/ask-first";
import { rememberAgent, resetPickedAgent } from "@/lib/ask-first-model";
import { getDriver, resetPrepare } from "@/lib/prepare/store";
import { mountApp, pickNewLinux, stubBrowserApis } from "./app-harness";

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
  setFlagOverride("spaces_ask_first_home", true);
  setFlagOverride("spaces_new_onboarding", true);
});
afterEach(() => {
  cleanup();
  setFlagOverride("spaces_ask_first_home", null);
  setFlagOverride("spaces_new_onboarding", null);
  resetPrepare();
});

const demo = { onboarded: true, noSpaces: true, keys: "several" as const };
/** The host reports the Warm start experiment on. */
const warmOn = (op: string) => (op === "experiments.get" ? { warmPools: true } : undefined);

const listText = () => document.querySelector("main")?.textContent ?? "";

describe.skipIf(!wasmBuilt)("with Warm start on, the warming home is not on any list until a person wrote into it", () => {
  it("is not on the Spaces list or in the sidebar count while warm and unasked, and its page is still reachable", { timeout: 60_000 }, async () => {
    const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo, intercept: warmOn });
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30_000 });
    // The host has the Space (it still counts toward its budget and usage); no list shows it.
    expect(getDriver()?.state?.homeSpaceId).toBe("local:home-claude-code");
    expect(listText()).not.toContain("home-claude-code");
    // Nowhere on the page: not the list, the sidebar's "Making" item, nor a tip about it.
    expect(document.body.textContent).not.toContain("home-claude-code");
    // Its own page is there (Home's Details link opens it).
    await act(async () => {
      await router.navigate({ to: "/spaces/$spaceId", params: { spaceId: "local:home-claude-code" } });
    });
    await waitFor(() => expect(document.body.textContent).toContain("home-claude-code"));
  });

  it("stays hidden after the app quits and opens again with the warm home and no message", { timeout: 60_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo, intercept: warmOn });
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30_000 });
    expect(JSON.parse(localStorage.getItem("cua.prepare.v1") ?? "{}").askedMs).toBeNull();
    cleanup();
    resetPrepare();
    // A new page load with the same storage; the host (the demo) still has the home.
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo, intercept: warmOn });
    await waitFor(() => expect(getDriver()?.state?.wanted).toBe(true));
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30_000 });
    expect(listText()).not.toContain("home-claude-code");
  });

  it("shows up once the first message is sent, even while that message is only queued", { timeout: 60_000 }, async () => {
    let release!: () => void;
    const hold = new Promise<void>((r) => (release = r));
    const { router } = await mountApp("/home", { failsCreates: false, listsGhost: false, hold, demo, intercept: warmOn });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(getDriver()?.state?.pendingId).toBeTruthy());
    expect(getDriver()?.state?.askedMs).toBeNull();
    await pickNewLinux();
    fireEvent.change(screen.getByLabelText("What should it do?"), { target: { value: "fix the build" } });
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await waitFor(() => expect(getDriver()?.state?.askedMs).not.toBeNull());
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\//));
    release();
    await act(async () => {
      await router.navigate({ to: "/spaces" });
    });
    await waitFor(() => expect(listText()).toContain("home-claude-code"), { timeout: 30_000 });
  });

  it("a home that cannot be made yet is not on a list and Home stays silent", { timeout: 60_000 }, async () => {
    await mountApp("/home", { failsCreates: true, listsGhost: false, demo, intercept: warmOn });
    await screen.findByLabelText("What should it do?");
    // A dropped connection is retried by itself: Home stays silent and the home stays off every list.
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("waiting"), { timeout: 30_000 });
    expect(document.querySelector("[data-prepare-line]")).toBeNull();
    expect(document.querySelector("[data-home-spaces]")?.textContent ?? "").not.toContain("home-claude-code");
    expect(listText()).not.toContain("home-claude-code");
  });

  it("a Space that is not the warming home is listed as before", { timeout: 60_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true, keys: "several" } });
    await waitFor(() => expect(listText()).toContain("ubuntu-build"), { timeout: 20_000 });
  });

  it("with Warm start off the same setup makes no home", { timeout: 60_000 }, async () => {
    const calls: string[] = [];
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo, intercept: (op) => (calls.push(op), undefined) });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1_500));
    });
    expect(calls).not.toContain("spaces.create");
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Warm start (always on) keeps the home that warms in the background for Home's prompt box, which claims the daemon's
// warm Space. It is invisible infrastructure until a person writes into it: not in the Computers tree, its counts, the
// sidebar or the palette, and not among the agents. Its own page and Home's preparing line still show it. The rule is
// the core's (`prepare.hidden`). Nothing starts while the first run shows or before it is over (CUA-3622 stays true
// for that: the first run makes nothing the person didn't ask for).

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { writeFirstRun } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetFirstRunCache } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetAskTasks } from "@/lib/ask-first";
import { rememberAgent, resetPickedAgent } from "@/lib/ask-first-model";
import { getDriver, holdWarmStart, resetPrepare } from "@/lib/prepare/store";
import { mountApp, pickNewLinux, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  holdWarmStart(false);
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

const demo = { onboarded: true, noSpaces: true, keys: "several" as const };
const HOME = "local:home-claude-code";

const mainText = () => document.querySelector("main")?.textContent ?? "";
const treeRows = () => [...document.querySelectorAll<HTMLElement>("[data-tree-row]")].map((r) => r.getAttribute("data-tree-row"));

describe.skipIf(!wasmBuilt)("with Warm start, the warming home is on no list until a person wrote into it", () => {
  it(
    "is not in the Computers tree, its summary or the sidebar count while warm and unasked, and its page is still reachable",
    { timeout: 60_000 },
    async () => {
      const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo });
      await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30_000 });
      // The host has the Space (it still counts toward its budget and usage); no list shows it.
      expect(getDriver()?.state?.homeSpaceId).toBe(HOME);
      await waitFor(() => expect(document.querySelector("[data-spaces-tree]")).toBeTruthy());
      expect(treeRows()).not.toContain(HOME);
      // Nowhere on the page: not the tree, its summary, the sidebar's "Making" item, nor a tip about it.
      expect(document.body.textContent).not.toContain("home-claude-code");
      // The summary counts Spaces without it: the same line as a host that holds no home.
      expect(document.querySelector("main")?.textContent ?? "").not.toMatch(/Spaces? made on/);
      // Its own page is there (Home's Details link opens it).
      await act(async () => {
        await router.navigate({ to: "/spaces/$spaceId", params: { spaceId: HOME } });
      });
      await waitFor(() => expect(document.body.textContent).toContain("home-claude-code"));
    },
  );

  it("stays hidden after the app quits and opens again with the warm home and no message", { timeout: 60_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo });
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30_000 });
    expect(JSON.parse(localStorage.getItem("cua.prepare.v1") ?? "{}").askedMs).toBeNull();
    cleanup();
    resetPrepare();
    // A new page load with the same storage; the host (the demo) still has the home.
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo });
    await waitFor(() => expect(getDriver()?.state?.wanted).toBe(true));
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30_000 });
    expect(treeRows()).not.toContain(HOME);
    expect(mainText()).not.toContain("home-claude-code");
  });

  it("shows up once the first message is sent, even while that message is only queued", { timeout: 60_000 }, async () => {
    let release!: () => void;
    const hold = new Promise<void>((r) => (release = r));
    const { router } = await mountApp("/home/ask", { failsCreates: false, listsGhost: false, hold, demo });
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
    await waitFor(() => expect(treeRows()).toContain(HOME), { timeout: 30_000 });
  });

  it("a home that cannot be made yet is on no list and Home stays silent", { timeout: 60_000 }, async () => {
    await mountApp("/home/ask", { failsCreates: true, listsGhost: false, demo });
    await screen.findByLabelText("What should it do?");
    // A dropped connection is retried by itself: Home stays silent and the home stays off every list.
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("waiting"), { timeout: 30_000 });
    expect(document.querySelector("[data-prepare-line]")).toBeNull();
    expect(document.querySelector("[data-home-spaces]")?.textContent ?? "").not.toContain("home-claude-code");
    expect(mainText()).not.toContain("home-claude-code");
  });

  it("a Space that is not the warming home is listed as before", { timeout: 60_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true, keys: "several" } });
    await waitFor(() => expect(mainText()).toContain("ubuntu-build"), { timeout: 20_000 });
  });

  it("makes nothing while the first run is not over", { timeout: 60_000 }, async () => {
    const calls: string[] = [];
    // Setup is still under way on screen 2 (a run saved on "next" now finishes on Home, so it is no longer "not over").
    writeFirstRun({ newRun: { screen: "where" } });
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { ...demo, signedIn: false, onboarded: false },
      intercept: (op) => (calls.push(op), undefined),
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1_500));
    });
    expect(calls).not.toContain("spaces.create");
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
  });

  it(
    "hides a home the host marks hidden even when this page holds no record of preparing it (cleared site data, an older build)",
    { timeout: 60_000 },
    async () => {
      holdWarmStart(true);
      const withHidden = (op: string, out: unknown) => {
        if (op !== "spaces.list") return out;
        const rows = out as Array<Record<string, unknown>>;
        return [...rows, { ...rows[0], id: HOME, name: "home-claude-code", hidden: true }];
      };
      await mountApp("/home/ask", { failsCreates: false, listsGhost: false, demo: { onboarded: true, keys: "several" }, answer: withHidden });
      await screen.findByLabelText("What should it do?");
      await waitFor(() => expect(document.querySelector("[data-home-spaces]")).toBeTruthy(), { timeout: 20_000 });
      expect(getDriver()?.state?.wanted ?? false).toBe(false);
      // Not a card on Home, and not in its "N running".
      expect(document.querySelector("[data-home-spaces]")?.textContent ?? "").not.toContain("home-claude-code");
      expect(document.body.textContent).not.toContain("home-claude-code");
      const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true, keys: "several" }, answer: withHidden });
      await waitFor(() => expect(document.querySelector("[data-spaces-tree]")).toBeTruthy());
      expect(treeRows()).not.toContain(HOME);
      expect(document.body.textContent).not.toContain("home-claude-code");
      void router;
    },
  );
});

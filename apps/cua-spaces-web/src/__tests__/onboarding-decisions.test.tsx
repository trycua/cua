// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Francesco's 7 Nov onboarding decisions, held in the new setup:
// - Ready screen chat pickup = A (CUA-3319): up to four recent Claude Code, Codex and Hermes chats are listed
//   without asking; their titles stay on this computer (no cache, no usage data, no relay), and a chat you
//   continue stays private to you.
// - Always-on machine = B (CUA-3691): opt-in only. Setup never preselects another computer, however always-on
//   it is: only this computer is picked until the person picks more.
// - Cua Driver in setup = A (CUA-2706): the one permissions step is the Cua Driver step (two Allow buttons and
//   Later); there is no separate "Meet Cua Driver" step.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { BridgeProvider, type DataAdapter } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { AppPermissions } from "@/bridge/app-permissions";
import type { AgentSetupRow } from "@/bridge/contracts/agents";
import { resetFirstRunCache, writeFirstRun } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { PickUpWhereYouLeftOff, PermissionsScreen } from "@/components/first-run-new/first-run-new";
import { PICK_UP_LIMIT } from "@/components/ask-first/pick-up";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { permissionsStepView } from "@/lib/app-permissions";
import { resetAskTasks } from "@/lib/ask-first";
import { NEW_RUN_EMPTY, NEW_RUN_SCREENS, readyComputers } from "@/lib/first-run-new";
import { resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  resetFirstRunCache();
  resetConnectedAppsState();
  resetAskTasks();
  resetPrepare();
});
afterEach(() => {
  cleanup();
  resetPrepare();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const all = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const heading = () => q("[data-onboarding-heading]")?.textContent ?? "";
const onlyCodex = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: r.agent === "codex" }));
type Calls = { op: string; args: unknown }[];

/** The demo's three chats (one per agent), as `agents.recentThreads` lists them. */
const TITLES = ["Kettle timer test fails on CI", "Checkout double charge", "Summarise my notes on the bakery lease"];

/** A host that lists more chats than asked for. */
function chattyHost(extra: number): DataAdapter {
  const demo = createDemoAdapter({ latencyMs: 1 });
  return {
    ...demo,
    subscribe: (l) => demo.subscribe(l),
    call: (async (op: string, args: never) => {
      const out = await demo.call(op as never, args);
      if (op !== "agents.recentThreads") return out;
      const rows = out as { agent: string; id: string; title: string; lastActiveMs: number }[];
      const more = Array.from({ length: extra }, (_, i) => ({
        ...rows[0]!,
        id: `extra-${i}`,
        title: `Older chat ${i + 1}`,
        lastActiveMs: rows[0]!.lastActiveMs - (i + 1) * 3_600_000,
      }));
      return [...rows, ...more];
    }) as DataAdapter["call"],
  };
}

describe("Ready screen chat pickup (A, CUA-3319)", () => {
  it("lists at most four chats, even when the host answers with more", async () => {
    expect(PICK_UP_LIMIT).toBe(4);
    render(
      <BridgeProvider adapter={chattyHost(5)} core={await testCore()}>
        <PickUpWhereYouLeftOff onContinue={() => {}} />
      </BridgeProvider>,
    );
    await waitFor(() => expect(all("[data-pick-up-row]").length).toBe(4));
  });

  it("lists Claude Code, Codex and Hermes chats", async () => {
    render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={await testCore()}>
        <PickUpWhereYouLeftOff onContinue={() => {}} />
      </BridgeProvider>,
    );
    await waitFor(() => expect(all("[data-pick-up-row]").length).toBe(3));
    expect(all("[data-pick-up-row]").map((r) => r.dataset.pickUpRow)).toEqual(["claude-code", "openai-codex", "hermes"]);
    for (const t of TITLES) expect(screen.getByText(t)).toBeTruthy();
  });

  it.skipIf(!wasmBuilt)(
    "shows on the setup's Ready screen without a flag or a question, and is hidden when there are no chats",
    { timeout: 60_000 },
    async () => {
      writeFirstRun({ newRun: { ...NEW_RUN_EMPTY, screen: "ready", agent: "codex", reachedStep: "ready" } });
      await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: false, noSpaces: true } });
      await waitFor(() => expect(heading()).toBe("Codex can now use this Mac"), { timeout: 15_000 });
      // Listed straight away: nothing to press first, and no Labs flag in the way. Only Codex's chats: Codex was picked.
      await waitFor(() => expect(all("[data-new-run=ready] [data-pick-up-row]").map((r) => r.dataset.pickUpRow)).toEqual(["openai-codex"]));
      expect(screen.queryByRole("button", { name: /show my recent chats/i })).toBeNull();
      cleanup();
      resetFirstRunCache();
      writeFirstRun({ newRun: { ...NEW_RUN_EMPTY, screen: "ready", agent: "codex", reachedStep: "ready" } });
      await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: false, noSpaces: true, noChats: true } });
      await waitFor(() => expect(heading()).toBe("Codex can now use this Mac"), { timeout: 15_000 });
      await new Promise((r) => setTimeout(r, 200));
      expect(q("[data-pick-up]")).toBeNull();
      // The rest of the screen is still there.
      expect(q("[data-new-run-command]")).not.toBeNull();
    },
  );

  it.skipIf(!wasmBuilt)(
    "keeps the titles on this computer: nothing stores, sends or posts them, and the continued chat stays private",
    { timeout: 60_000 },
    async () => {
      const calls: Calls = [];
      writeFirstRun({ newRun: { ...NEW_RUN_EMPTY, screen: "ready", agent: "codex", reachedStep: "ready" } });
      const { router } = await mountApp("/onboarding", {
        failsCreates: false,
        listsGhost: false,
        demo: { signedIn: true, onboarded: false, noSpaces: true },
        intercept: (op, args) => (calls.push({ op, args }), undefined),
      });
      await waitFor(() => expect(all("[data-pick-up-row]").length).toBe(1), { timeout: 15_000 });
      // Listed, not kept: no title in this device's storage before one is picked.
      const stored = () =>
        Object.keys(localStorage)
          .map((k) => localStorage.getItem(k) ?? "")
          .join("\n");
      for (const t of TITLES) expect(stored()).not.toContain(t);
      fireEvent.click(all("[data-pick-up-continue]")[0]!);
      await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/home\/.+/), { timeout: 15_000 });
      await new Promise((r) => setTimeout(r, 300));
      // The picked chat is the only one this device remembers (its own Home task); the others are not kept.
      expect(stored()).toContain(TITLES[1]);
      for (const t of [TITLES[0]!, TITLES[2]!]) expect(stored()).not.toContain(t);
      // No call carries a title: not usage data, not the relay, not a Space thread.
      for (const c of calls) {
        const sent = JSON.stringify(c.args ?? null);
        for (const t of TITLES) expect(sent, `${c.op} sent a chat title`).not.toContain(t);
      }
      expect(calls.some((c) => c.op === "telemetry.track")).toBe(true);
      // Private: nothing is posted to a Space thread, and nothing runs until the person sends.
      expect(calls.some((c) => c.op === "thread.post")).toBe(false);
      expect(calls.some((c) => c.op === "agents.start")).toBe(false);
    },
  );
});

describe("Always-on machine (B, CUA-3691), updated 8 Nov 2026: no step to pick computers", () => {
  // Francesco took "Pick where your agents work" out of setup: agents can use every ready computer, an always-on
  // server included; Settings changes that. Nothing is made for it either.
  it("an always-on server in the account is one of the computers agents can use", () => {
    expect(
      readyComputers([
        { name: "Francesco's MacBook", online: true, current: true },
        { name: "Build server", online: true, current: false },
        { name: "Mac mini", online: true, current: false },
      ]),
    ).toEqual(["this", "Build server", "Mac mini"]);
    expect(NEW_RUN_SCREENS).not.toContain("where");
  });
});

describe("Cua Driver in setup (A, CUA-2706): the permissions step is the Cua Driver step", () => {
  const SCREEN = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
  const AX = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
  const missing: AppPermissions = {
    appName: "Cua Spaces",
    permissions: [
      { id: "screen", granted: false, settingsUrl: SCREEN },
      { id: "accessibility", granted: false, settingsUrl: AX },
    ],
  };

  it("is one screen with two Allow buttons and a way on without them, and names Cua Driver", () => {
    const allow = vi.fn(async () => {});
    const onContinue = vi.fn();
    render(
      <PermissionsScreen view={permissionsStepView({ appName: "Cua Spaces", allGranted: false })} perms={{ data: missing, allow }} onContinue={onContinue} />,
    );
    const buttons = screen.getAllByRole("button").map((b) => b.textContent);
    expect(buttons).toEqual(["Allow", "Allow", "Continue without them", "Still off: See the screen and Use the mouse and keyboard"]);
    expect(all("[data-app-permission]").map((r) => r.dataset.appPermission)).toEqual(["screen", "accessibility"]);
    expect(document.body.textContent).toContain("Cua Driver");
    fireEvent.click(screen.getByRole("button", { name: "Continue without them" }));
    expect(onContinue).toHaveBeenCalledTimes(1);
  });

  it("has no separate Cua Driver step among the setup's screens", () => {
    expect(NEW_RUN_SCREENS.filter((s) => /driver/i.test(s))).toEqual([]);
    expect(NEW_RUN_SCREENS.filter((s) => s === "permissions")).toHaveLength(1);
  });

  it.skipIf(!wasmBuilt)("a whole run on a Mac shows the permissions step once and never a 'Meet Cua Driver' screen", { timeout: 60_000 }, async () => {
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      answer: (op, out) => (op === "agents.setup" || op === "agents.configure" ? onlyCodex(out as AgentSetupRow[]) : out),
    });
    const seen: string[] = [];
    const note = () => {
      const h = heading();
      if (h && seen.at(-1) !== h) seen.push(h);
    };
    await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
    note();
    fireEvent.click(q("[data-onboarding-later]")!);
    await waitFor(() => expect(heading()).toBe("Let Cua Spaces see and use apps"), { timeout: 15_000 });
    note();
    await waitFor(() => expect(all("[data-app-permission]").length).toBe(2));
    fireEvent.click(q("[data-onboarding-primary]")!);
    await waitFor(() => expect(heading()).toBe("Which agents do you use?"), { timeout: 15_000 });
    note();
    fireEvent.click(q("[data-onboarding-primary]")!);
    await waitFor(() => expect(heading()).toBe("Codex can now use this Mac"), { timeout: 15_000 });
    note();
    expect(seen.filter((h) => /see and use apps/.test(h))).toHaveLength(1);
    expect(seen.some((h) => /meet cua driver/i.test(h))).toBe(false);
  });
});

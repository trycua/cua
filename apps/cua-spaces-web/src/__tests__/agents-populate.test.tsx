// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Agents page fills in at once (CUA-2849): from the last visit's list
// while the Spaces are read again, row by row as each Space answers, with a
// Space that never answers holding up nothing; and a long DM opens on its
// newest messages with the older ones a click away.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
beforeEach(() => localStorage.clear());
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const HERMES = "acct_ada/hermes/hermes";
const CODEX = "acct_ada/codex/codex";
const UBUNTU = "relay:linux-box/ubuntu-build";
const never = () => new Promise<never>(() => undefined);
const found = (s: string, timeout = 5000) =>
  waitFor(
    () => {
      const el = q(s);
      expect(el).not.toBeNull();
      return el!;
    },
    { timeout },
  );

describe("the Agents page while Spaces are slow", () => {
  it("draws the Spaces that answered and says it is still checking the one that has not", async () => {
    const t0 = Date.now();
    await mountApp("/agents", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => (op.startsWith("thread.") && (args as { spaceId?: string }).spaceId === UBUNTU ? never() : undefined),
    });
    // Codex lives in design-review: its row and Hermes's (also in release-checks) do not wait for ubuntu-build.
    await found(`[data-chat='${CODEX}']`);
    await found(`[data-chat='${HERMES}']`);
    expect(Date.now() - t0).toBeLessThan(3000);
    const loading = await found("[data-agents-loading]");
    expect(loading.textContent).toBe("Checking 1 Space");
    // Hermes is only in the Spaces that answered.
    await waitFor(() => expect(q(`[data-chat='${HERMES}']`)).not.toBeNull());
  });

  it("draws the last visit's list before any Space has answered", async () => {
    const first = await mountApp("/agents", { failsCreates: false, listsGhost: false });
    await found(`[data-chat='${CODEX}']`);
    // Let the pass finish and the list be kept.
    await waitFor(() => expect(localStorage.getItem("cua.agents.lastKnown.v1")).toContain("design-review"));
    expect(first.router).toBeDefined();
    cleanup();
    // The next visit: not one thread call is answered, and the rows are there.
    await mountApp("/agents", { failsCreates: false, listsGhost: false, intercept: (op) => (op.startsWith("thread.") ? never() : undefined) });
    await found(`[data-chat='${CODEX}']`, 2000);
    expect(q(`[data-chat='${CODEX}'] [data-chat-line]`)?.textContent).toBe("Waiting for you in design-review");
    // And it says it is checking, quietly.
    await found("[data-agents-loading]");
  });

  it("a DM opens on the newest messages; earlier ones are a click away", async () => {
    const { router, demo } = await mountApp("/", { failsCreates: false, listsGhost: false });
    for (let i = 1; i <= 70; i++) await demo.call("thread.post", { spaceId: UBUNTU, text: `@hermes message ${i}` });
    await act(async () => {
      await router.navigate({ to: "/agents", search: { agent: HERMES, space: UBUNTU } });
    });
    await found("[data-dm-log] > div > *");
    // The log's rows (the button above them is not one).
    const shown = () => qa("[data-dm-log] > div > *").filter((el) => !el.hasAttribute("data-dm-older")).length;
    await waitFor(() => expect(q("[data-dm-older]")).not.toBeNull(), { timeout: 5000 });
    const first = shown();
    expect(first).toBeGreaterThan(5);
    expect(first).toBeLessThan(45);
    // 70 tasks and their replies are 140 entries: one page of each Space is the newest 40.
    expect(q("[data-dm-log]")?.textContent).not.toContain("message 1 ");
    await act(async () => {
      fireEvent.click(q("[data-dm-older]")!);
      await new Promise((r) => setTimeout(r, 50));
    });
    await waitFor(() => expect(shown()).toBeGreaterThan(first));
  });

  it("one Space of an agent's DM never answering does not hold the other's messages", async () => {
    await mountApp(`/agents?agent=${encodeURIComponent(HERMES)}`, {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => (op === "thread.read" && (args as { spaceId?: string; with?: string }).spaceId === UBUNTU && (args as { with?: string }).with ? never() : undefined),
    });
    // release-checks' messages show though ubuntu-build's DM is still loading.
    await found(`[data-dm='${HERMES}'][data-dm-space='all']`);
    await waitFor(() => expect(q("[data-dm-log]")?.textContent?.length ?? 0).toBeGreaterThan(0), { timeout: 3000 });
    expect(q("[data-dm-empty]")).toBeNull();
  });
});

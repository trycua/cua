// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// How long the Agents page takes to fill in on an account like a busy one
// (CUA-2849): every Space's thread answers after the time its machine takes,
// one is far slower, one is off. Rows must show as their Space answers, and
// the slow one must hold up nothing. The numbers are on the console; the
// thresholds are generous, they fail on a page that waits for the slowest
// Space (before: the first row came when the slowest answered).

import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
beforeEach(() => localStorage.clear());
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const CODEX = "acct_ada/codex/codex";
const HERMES = "acct_ada/hermes/hermes";
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
/** What each Space's machine takes to answer a thread call (ms); `off` is a machine that is off. */
const TAKES: Record<string, number | "off"> = {
  "local:design-review": 20,
  "local:agent-sandbox": 20,
  "relay:mac-mini/release-checks": 300,
  "relay:mac-mini/qa-windows": 300,
  "relay:linux-box/ubuntu-build": 600,
  "relay:linux-box/windows-legacy": 600,
  "relay:space-sam-qa-shared": 1500,
  "relay:space-priya-checkout": 8000,
  "relay:space-qa-regression": "off",
};

async function visit(label: string): Promise<Record<string, number | null>> {
  {
    const t0 = Date.now();
    // The host's own answer, after the time that Space's machine takes.
    const host: { demo?: { call(op: never, args: never): Promise<unknown> } } = {};
    const app = await mountApp("/agents", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        const space = (args as { spaceId?: string }).spaceId;
        if (!op.startsWith("thread.") || !space || !(space in TAKES)) return undefined;
        const t = TAKES[space]!;
        if (t === "off") return Promise.reject(new Error("machine is offline"));
        return sleep(t).then(() => host.demo!.call(op as never, args as never));
      },
    });
    host.demo = app.demo;
    const seen: Record<string, number> = {};
    const settled = () => (q("[data-agents-loading]") ? null : Date.now() - t0);
    let done: number | null = null;
    while (Date.now() - t0 < 12_000) {
      for (const [name, sel] of [["codex row", `[data-chat='${CODEX}']`], ["hermes row", `[data-chat='${HERMES}']`]] as const) if (!(name in seen) && q(sel)) seen[name] = Date.now() - t0;
      if ("codex row" in seen && "hermes row" in seen && (done = settled()) !== null && seen["codex row"] !== undefined) break;
      await sleep(10);
    }
    console.log(`agents page timeline, ${label} (ms from open): ${JSON.stringify({ ...seen, checkingGone: done })}`);
    return { ...seen, checkingGone: done };
  }
}

describe("the Agents page on a busy account", () => {
  it("shows rows as their Spaces answer, not when the slowest one does", async () => {
    const t = await visit("cold");
    expect(t["codex row"], "first row").toBeDefined();
    expect(t["codex row"]!).toBeLessThan(1500);
    expect(t["hermes row"]!).toBeLessThan(2500);
  }, 30_000);

  it("the next visit draws from the last one at once, before the slow Spaces answer", async () => {
    await visit("first visit");
    cleanup();
    const t = await visit("warm");
    expect(t["codex row"]!).toBeLessThan(400);
    expect(t["hermes row"]!).toBeLessThan(400);
  }, 60_000);
});

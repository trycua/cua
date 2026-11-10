// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The only Space the app makes by itself. After the first run (Warm start) it warms one hidden home, the Space Home's
// prompt box writes into; it makes it once, finds it again when the app opens, and rebuilds it when it was removed. It
// never makes a second one, and every other Space is made only when the person chooses it ("New computer" for a job).

import { act, cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { writeFirstRun } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectState } from "@/hooks/use-connectors";
import { resetAskTasks } from "@/lib/ask-first";
import { getDriver, holdWarmStart, resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  holdWarmStart(false);
  localStorage.clear();
  // Set up with this app's first run: the home warms only after one.
  writeFirstRun({ newRun: { screen: "next" } });
  resetAskTasks();
  resetConnectState();
  resetPrepare();
});
afterEach(() => {
  cleanup();
  resetPrepare();
  vi.unstubAllEnvs();
});

type Call = { op: string; args: Record<string, unknown> };
const record = (calls: Call[]) => (op: string, args: unknown) => (calls.push({ op, args: args as Record<string, unknown> }), undefined);
const settle = () =>
  act(async () => {
    await new Promise((r) => setTimeout(r, 1_500));
  });
const creates = (calls: Call[]) => calls.filter((c) => c.op === "spaces.create");

const HOME_NAME = "home-claude-code";
const homeCreates = (calls: Call[]) => creates(calls).filter((c) => JSON.stringify(c.args).includes(HOME_NAME));

describe.skipIf(!wasmBuilt)("the app makes the warm home and no other Space on its own", () => {
  it("makes exactly one Space after the first run: the hidden home", { timeout: 40_000 }, async () => {
    const calls: Call[] = [];
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true, noSpaces: true }, intercept: record(calls) });
    await waitFor(() => expect(creates(calls)).toHaveLength(1), { timeout: 20_000 });
    await settle();
    expect(creates(calls)).toHaveLength(1);
    expect(homeCreates(calls)).toHaveLength(1);
    // Labelled hidden and off the relay: no list shows it and the phone does not list it.
    expect(JSON.stringify(creates(calls)[0]!.args)).toContain("cua.hidden");
    expect(getDriver()?.state?.wanted).toBe(true);
  });

  it("rebuilds the home once when the app opens with a record whose home is gone", { timeout: 40_000 }, async () => {
    localStorage.setItem(
      "cua.prepare.v1",
      JSON.stringify({
        wanted: true,
        harness: "claude-code",
        pendingId: "pending:old",
        homeSpaceId: "local:home-claude-code",
        readyMs: 1,
        messages: [],
        askedMs: null,
        online: true,
      }),
    );
    const calls: Call[] = [];
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, demo: { onboarded: true, noSpaces: true }, intercept: record(calls) });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(homeCreates(calls)).toHaveLength(1), { timeout: 20_000 });
    await settle();
    expect(homeCreates(calls)).toHaveLength(1);
  });

  it("makes nothing before the first run is over", { timeout: 40_000 }, async () => {
    localStorage.clear();
    writeFirstRun(null);
    const calls: Call[] = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: record(calls),
    });
    await settle();
    expect(creates(calls)).toHaveLength(0);
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
  });
});

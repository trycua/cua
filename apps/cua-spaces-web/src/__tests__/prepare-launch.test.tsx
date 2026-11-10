// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// No hidden Space. The app never makes a Space the person did not choose: not after the first run, not when it
// opens again or updates (an old saved "preparing" record included), and not when a home was removed. A Linux Space
// is made only when the person targets "New computer" for a job.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { writeFirstRun } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectState } from "@/hooks/use-connectors";
import { resetAskTasks } from "@/lib/ask-first";
import { getDriver, resetPrepare } from "@/lib/prepare/store";
import { mountApp, pickNewLinux, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
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

describe.skipIf(!wasmBuilt)("no Space is made unless the person chooses one", () => {
  it("makes nothing after the first run, on Spaces or on Home", { timeout: 40_000 }, async () => {
    const calls: Call[] = [];
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true, noSpaces: true }, intercept: record(calls) });
    await settle();
    expect(creates(calls)).toHaveLength(0);
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
    cleanup();
    resetPrepare();
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { onboarded: true, noSpaces: true }, intercept: record(calls) });
    await screen.findByLabelText("What should it do?");
    await settle();
    expect(creates(calls)).toHaveLength(0);
    expect(calls.some((c) => c.op === "thread.agentAdd")).toBe(false);
  });

  it("makes nothing when the app opens with a record an older version saved, wanted or with its home gone", { timeout: 40_000 }, async () => {
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
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { onboarded: true, noSpaces: true }, intercept: record(calls) });
    await screen.findByLabelText("What should it do?");
    await settle();
    expect(creates(calls)).toHaveLength(0);
    expect(calls.some((c) => c.op === "thread.agentAdd")).toBe(false);
  });

  it("makes one Space when the person targets New computer and sends a job", { timeout: 60_000 }, async () => {
    const calls: Call[] = [];
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { onboarded: true, noSpaces: true }, intercept: record(calls) });
    await screen.findByLabelText("What should it do?");
    await settle();
    expect(creates(calls)).toHaveLength(0);
    await pickNewLinux();
    fireEvent.change(screen.getByLabelText("What should it do?"), { target: { value: "fix the build" } });
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await waitFor(() => expect(creates(calls)).toHaveLength(1), { timeout: 20_000 });
    // It is the person's Space: it carries no hidden or off-the-relay label.
    expect(JSON.stringify(creates(calls)[0]!.args)).not.toContain("cua.hidden");
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { OnboardingAction, OnboardingFlowState } from "../contracts/onboarding";
import { onboardingInitial, onboardingReduce } from "../onboarding";
import { sanitizeSignals } from "../ops/telemetry";
import { telemetryLaunched, telemetryOnboarding, type TelemetrySignal } from "../telemetry";
import { testCore, wasmBuilt } from "./testCore";

const words = (s: TelemetrySignal[]) =>
  s.map((x) => (x.type === "step" ? x.step : x.type === "onboarding-page" ? `${x.page} ${x.action}` : x.type));

async function run(actions: OnboardingAction[]): Promise<string[][]> {
  const core = await testCore();
  const out: string[][] = [];
  let s: OnboardingFlowState = onboardingInitial(core, null);
  for (const a of actions) {
    const signals: TelemetrySignal[] = [];
    s = onboardingReduce(core, s, a, {
      menuBar: false,
      observe: (before, action) => signals.push(...telemetryOnboarding(core, before, action)),
    });
    out.push(words(signals));
  }
  return out;
}

describe.skipIf(!wasmBuilt)("first-run usage events (wasm core)", () => {
  it("counts the run when Welcome shows, once the notice was shown on this machine", async () => {
    const frames = await run([
      { type: "telemetry-loaded", telemetry: { enabled: true, lockedBy: null, noticeShown: true } },
      { type: "welcome-shown" },
      { type: "start" },
      { type: "back" },
      { type: "welcome-shown" },
      { type: "start" },
    ]);
    expect(frames).toEqual([
      [],
      ["onboarding_shown", "welcome shown"],
      ["welcome completed", "signin shown"],
      ["signin back", "welcome shown"],
      [],
      ["welcome completed", "signin shown"],
    ]);
  });

  it("sends nothing before Welcome is left on a machine that never showed the notice", async () => {
    const frames = await run([
      { type: "telemetry-loaded", telemetry: { enabled: true, lockedBy: null, noticeShown: false } },
      { type: "welcome-shown" },
      { type: "start" },
    ]);
    expect(frames).toEqual([[], [], ["onboarding_shown", "welcome shown", "welcome completed", "signin shown"]]);
  });

  it("says on app_launched whether the first run is due", async () => {
    const core = await testCore();
    expect(telemetryLaunched(core, true)).toEqual([{ type: "launched", onboardingEligible: true }]);
    expect(telemetryLaunched(core)).toEqual([{ type: "launched", onboardingEligible: null }]);
  });
});

describe("the launched signal crossing the bridge", () => {
  it("keeps a flag or null and drops anything else", () => {
    expect(sanitizeSignals([{ type: "launched", onboardingEligible: false }])).toEqual([{ type: "launched", onboardingEligible: false }]);
    expect(sanitizeSignals([{ type: "launched", onboardingEligible: null }])).toHaveLength(1);
    expect(sanitizeSignals([{ type: "launched", onboardingEligible: "maya@example.com" }])).toEqual([]);
  });
});

describe.skipIf(!wasmBuilt)("first-run usage events from the web pages", () => {
  it("never reports the Menu bar page it passes through, and counts Set up later", async () => {
    const { act, render, waitFor } = await import("@testing-library/react");
    const { createElement } = await import("react");
    const { BridgeProvider, useOnboarding } = await import("../index");
    const { createDemoAdapter } = await import("../adapters/demo");
    const { resetSavedOnboardingCache } = await import("../onboarding");
    localStorage.clear();
    resetSavedOnboardingCache();
    const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 1, signedIn: false, onboarded: false });
    const probe = {} as { flow: ReturnType<typeof useOnboarding> };
    const P = () => {
      probe.flow = useOnboarding();
      return null;
    };
    render(createElement(BridgeProvider, { adapter, core: await testCore(), storeOptions: { tickMs: 5 } }, createElement(P)));
    await waitFor(() => expect(probe.flow.view?.usage).toBeTruthy());
    act(() => probe.flow.send({ type: "start" }));
    act(() => probe.flow.send({ type: "signin-done" }));
    act(() => probe.flow.send({ type: "agents-done", configured: [] }));
    act(() => probe.flow.skip());
    const sent = () => words(adapter.state.telemetry);
    await waitFor(() => expect(sent()).toContain("onboarding_skipped"));
    expect(sent().filter((w) => w.startsWith("presentation"))).toEqual([]);
    expect(sent()).toContain("agents skipped");
    expect(sent()).toContain("this_machine shown");
    expect(adapter.state.telemetry).toContainEqual({ type: "onboarding-page", page: "this_machine", action: "skipped", choice: "later" });
  });
});

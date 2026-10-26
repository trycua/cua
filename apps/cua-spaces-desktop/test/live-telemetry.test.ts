// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The app's usage events on a first run: the SDK sends nothing before the
// app records that its first-run notice was shown, so the app holds the
// first session's signals until Welcome is left, then sends them in order
// (CUA-1739: Windows and Linux sent no first run, sign-in or Space events).
import { describe, expect, it } from "vitest";
import { hostRecorded, leftWelcome } from "../src/bridge/telemetry";
import { createErrorVariant } from "../src/model/errors";
import { LiveTelemetry } from "../src/model/services";
import type { Native } from "../src/native/load";

type Signal = { tag: string };

function fakeNative(o: { noticeShown?: boolean; enabled?: boolean; failAck?: boolean } = {}) {
  const state = { noticeShown: o.noticeShown ?? false, enabled: o.enabled ?? true };
  const recorded: Signal[][] = [];
  const welcomeLeft: boolean[] = [];
  const status = () => ({ enabled: state.enabled, source: "default", sourceKind: "default", noticeShown: state.noticeShown, noticeText: "", docsUrl: "" });
  const native = {
    telemetryStatus: status,
    telemetrySetEnabled: (on: boolean) => {
      state.enabled = on;
      return status();
    },
    appTelemetryRecord: (signals: Signal[]) => {
      recorded.push(signals);
      return signals.length;
    },
    appTelemetryWelcomeLeft: (on: boolean) => {
      if (o.failAck) throw new Error("no home");
      welcomeLeft.push(on);
      state.noticeShown = true;
    },
  } as unknown as Native;
  return { native, state, recorded, welcomeLeft };
}

const sig = (tag: string) => ({ tag }) as never;

describe("LiveTelemetry: the one-time linking notice", () => {
  it("reads the SDK's words and whether it is due, and acknowledges through the SDK", () => {
    const acknowledged: number[] = [];
    let due = true;
    const native = {
      telemetryLinkNotice: () => ({
        title: "We've updated how usage data works",
        body: "Cua collects usage data.",
        okLabel: "Got it",
        turnOffLabel: "Turn off usage data",
        detailsLabel: "What is collected",
        docsUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry",
      }),
      telemetryLinkNoticeDue: () => due,
      telemetryAcknowledgeLinkNotice: () => {
        acknowledged.push(1);
        due = false;
      },
    } as unknown as Native;
    const t = new LiveTelemetry(native);
    expect(t.linkNotice()).toEqual({
      due: true,
      title: "We've updated how usage data works",
      body: "Cua collects usage data.",
      okLabel: "Got it",
      turnOffLabel: "Turn off usage data",
      detailsLabel: "What is collected",
      docsUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry",
    });
    t.acknowledgeLinkNotice();
    expect(acknowledged).toEqual([1]);
    expect(t.linkNotice().due).toBe(false);
  });
});

describe("LiveTelemetry before and after the first-run notice", () => {
  it("holds the first session's signals until the notice was seen, then sends them in order", () => {
    const f = fakeNative();
    const t = new LiveTelemetry(f.native);
    t.record([sig("Launched"), sig("OnboardingPage")]);
    t.record([sig("Step")]);
    expect(f.recorded).toEqual([]);
    t.acknowledgeNotice();
    expect(f.welcomeLeft).toEqual([true]);
    expect(f.recorded).toEqual([[sig("Launched"), sig("OnboardingPage"), sig("Step")]]);
    // Afterwards signals go straight through.
    t.record([sig("SpaceCreateStarted")]);
    expect(f.recorded.at(-1)).toEqual([sig("SpaceCreateStarted")]);
    // Acknowledging again writes nothing and sends nothing twice.
    t.acknowledgeNotice();
    expect(f.welcomeLeft).toEqual([true]);
    expect(f.recorded).toHaveLength(2);
  });

  it("sends at once on a machine that already showed the notice", () => {
    const f = fakeNative({ noticeShown: true });
    const t = new LiveTelemetry(f.native);
    t.record([sig("Feature")]);
    expect(f.recorded).toEqual([[sig("Feature")]]);
    t.acknowledgeNotice();
    expect(f.welcomeLeft).toEqual([]);
  });

  it("drops what it held when usage data is turned off on Welcome", () => {
    const f = fakeNative();
    const t = new LiveTelemetry(f.native);
    t.record([sig("OnboardingPage")]);
    t.setEnabled(false);
    t.acknowledgeNotice();
    // The notice is recorded with the switch as it is (off); nothing is sent.
    expect(f.welcomeLeft).toEqual([false]);
    expect(f.recorded).toEqual([]);
  });

  it("keeps holding when the notice could not be recorded", () => {
    const f = fakeNative({ failAck: true });
    const t = new LiveTelemetry(f.native);
    t.record([sig("Step")]);
    t.acknowledgeNotice();
    expect(f.recorded).toEqual([]);
  });

  it("holds a bounded number of signals", () => {
    const f = fakeNative();
    const t = new LiveTelemetry(f.native);
    for (let i = 0; i < 250; i++) t.record([sig(`S${i}`)]);
    t.acknowledgeNotice();
    expect(f.recorded[0]).toHaveLength(200);
    expect(f.recorded[0]![0]).toEqual(sig("S50"));
  });
});

describe("leaving Welcome", () => {
  it("is any first-run page but Welcome being shown, or Set up later", () => {
    expect(leftWelcome({ type: "onboarding-page", page: "welcome", action: "shown", choice: "none" })).toBe(false);
    expect(leftWelcome({ type: "step", step: "onboarding_shown", ok: true })).toBe(false);
    expect(leftWelcome({ type: "feature", feature: "space_open" })).toBe(false);
    expect(leftWelcome({ type: "onboarding-page", page: "welcome", action: "step_viewed", choice: "none" })).toBe(false);
    expect(leftWelcome({ type: "onboarding-page", page: "welcome", action: "step_completed", choice: "none" })).toBe(true);
    expect(leftWelcome({ type: "onboarding-page", page: "welcome", action: "completed", choice: "none" })).toBe(true);
    expect(leftWelcome({ type: "onboarding-page", page: "signin", action: "shown", choice: "none" })).toBe(true);
    expect(leftWelcome({ type: "step", step: "onboarding_skipped", ok: true })).toBe(true);
    expect(leftWelcome(null)).toBe(false);
  });
});

describe("a create's signals from the page", () => {
  // The host records them from the create it runs (AppModel.sendCreate):
  // the page's copies counted every Windows and Linux create twice.
  it("are left to the host; the page's other signals are not", () => {
    for (const raw of [
      { type: "space-create", outcome: "error" },
      { type: "space-create-started" },
      { type: "feature", feature: "space_create_local" },
      { type: "feature", feature: "space_create_cloud" },
      { type: "step", step: "first_space_created", ok: true },
      { type: "step", step: "first_space_ready", ok: true },
    ]) {
      expect(hostRecorded(raw)).toBe(true);
    }
    for (const raw of [
      { type: "feature", feature: "space_open" },
      { type: "step", step: "checklist_create_space", ok: true },
      { type: "space-wizard", action: "opened" },
      { type: "onboarding-page", page: "done", action: "shown", choice: "none" },
      null,
      [],
    ]) {
      expect(hostRecorded(raw)).toBe(false);
    }
  });
});

describe("a failed create's error case", () => {
  const sdkError = (tag: string) => Object.assign(new Error(`CuaError.${tag}: boom`), { [Symbol.for("typeName")]: "CuaError", tag });
  it("is the SDK's variant, as the SwiftUI app records it", () => {
    expect(createErrorVariant(sdkError("DaemonNotRunning"))).toBe("DaemonNotRunning");
    expect(createErrorVariant(sdkError("Runtime"))).toBe("Runtime");
  });
  it("is empty for anything else, so the core reads the cause from the text", () => {
    expect(createErrorVariant(new Error("socket hang up"))).toBe("");
    expect(createErrorVariant(sdkError("not a word"))).toBe("");
    expect(createErrorVariant("text")).toBe("");
  });
});

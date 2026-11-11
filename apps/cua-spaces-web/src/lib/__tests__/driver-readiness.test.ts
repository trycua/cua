// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { driverReadiness, readinessLede, repairOutcome } from "../app-permissions";

const on = { available: true, allGranted: true };
const off = { available: true, allGranted: false };
const perms = (screen: boolean, accessibility: boolean) => [
  { id: "screen" as const, granted: screen, settingsUrl: "x-apple.systempreferences:screen" },
  { id: "accessibility" as const, granted: accessibility, settingsUrl: "x-apple.systempreferences:ax" },
];

describe("Ready only after Cua Driver runs with its permissions", () => {
  it("waits for the permissions first", () => {
    expect(driverReadiness({ permissions: off, driver: { state: "ready" } })).toBe("permissions");
  });
  it("waits for a running driver", () => {
    expect(driverReadiness({ permissions: on, driver: { state: "starting" } })).toBe("starting");
    expect(driverReadiness({ permissions: on, driver: { state: "failed" } })).toBe("driver");
    expect(driverReadiness({ permissions: on, driver: { state: "stopped" } })).toBe("driver");
  });
  it("is ready with both on and the driver answering", () => {
    expect(driverReadiness({ permissions: on, driver: { state: "ready" } })).toBe("ready");
  });
  it("has nothing to wait for where there is no driver or no permission", () => {
    expect(driverReadiness({ permissions: on, driver: null })).toBe("ready");
    expect(driverReadiness({ permissions: { available: false, allGranted: false }, driver: { state: "failed" } })).toBe("ready");
  });
  it("says what is missing in plain words", () => {
    expect(readinessLede("permissions", "Cua Spaces", null)).toContain("two switches");
    expect(readinessLede("driver", "Cua Spaces", "Cua Driver didn't start: boom")).toBe("Cua Driver didn't start: boom Repair starts it again.");
  });
});

describe("Repair outcomes", () => {
  const base = { state: "ready", appName: "Cua Spaces", permissions: perms(true, true), lastError: null, lastTest: { passed: true, failure: null } };
  it("ok when running and the test passed", () => {
    expect(repairOutcome(base).kind).toBe("ok");
  });
  it("opens the guide for the first missing permission", () => {
    const out = repairOutcome({
      ...base,
      state: "needs-permission",
      permissions: perms(false, true),
      lastTest: { passed: false, failure: "Screen Recording is off for Cua Spaces" },
    });
    expect(out.kind).toBe("permission");
    expect(out.settingsUrl).toBe("x-apple.systempreferences:screen");
    expect(out.description).toContain("turn on Cua Spaces");
  });
  it("fails with the reason and one next step", () => {
    const out = repairOutcome({ ...base, state: "failed", lastError: "Cua Driver didn't start: exit 1" });
    expect(out.kind).toBe("failed");
    expect(out.description).toBe("Cua Driver didn't start: exit 1. Quit and reopen Cua Spaces, then try Repair again.");
  });
  it("says so when agents are paused", () => {
    expect(repairOutcome({ ...base, state: "paused" }).title).toBe("Cua Driver is off");
  });
});

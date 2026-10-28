// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { devicesPageState, shellDevicesBanner } from "./devices";

describe("devicesPageState", () => {
  it("never leaves the Devices tab blank", () => {
    expect(devicesPageState({ error: null, data: undefined }, true)).toBe("loading");
    expect(devicesPageState({ error: new Error("relay timed out"), data: undefined }, true)).toBe("error");
    expect(devicesPageState({ error: null, data: { view: null } }, true)).toBe("no-view");
    expect(devicesPageState({ error: null, data: { view: {} } }, true)).toBe("ready");
    expect(devicesPageState({ error: null, data: undefined }, false)).toBe("signed-out");
    expect(devicesPageState({ unsupported: true, error: null }, true)).toBe("unsupported");
  });
});

describe("shellDevicesBanner", () => {
  const banner = { tone: "warning" as const, text: "Enroll this device within 3 days to keep reaching your machines.", action: "enroll" as const, actionLabel: "Enroll…" };
  const read = (input: { devices?: { id: string; state: string }[]; readError?: string | null }) => ({ input, view: { banner } });

  it("shows the core's bar once signed in and read", () => {
    expect(shellDevicesBanner(read({ devices: [] }), true)).toBe(banner);
    expect(shellDevicesBanner(read({ devices: [] }), false)).toBeNull();
    expect(shellDevicesBanner(undefined, true)).toBeNull();
    expect(shellDevicesBanner({ input: {}, view: { banner: null } }, true)).toBeNull();
  });

  it("shows none when the first read was refused, as the SwiftUI app has no list then", () => {
    expect(shellDevicesBanner(read({ devices: [], readError: "not enrolled" }), true)).toBeNull();
    expect(shellDevicesBanner(read({ readError: "not enrolled" }), true)).toBeNull();
    // A later read failed: the list from before still stands.
    expect(shellDevicesBanner(read({ devices: [{ id: "d1", state: "expired" }], readError: "timed out" }), true)).toBe(banner);
  });
});

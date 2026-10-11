// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { devicesPageState } from "./devices";

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

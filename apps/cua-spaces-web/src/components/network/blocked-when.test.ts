// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { blockedWhen } from "./network-sheet";

describe("when a connection was blocked", () => {
  const now = 1_000_000_000_000;
  it("says just now, minutes, hours and days", () => {
    expect(blockedWhen(now - 5_000, now)).toBe("Blocked just now");
    expect(blockedWhen(now - 60_000, now)).toBe("Blocked 1 minute ago");
    expect(blockedWhen(now - 3 * 60_000, now)).toBe("Blocked 3 minutes ago");
    expect(blockedWhen(now - 2 * 3_600_000, now)).toBe("Blocked 2 hours ago");
    expect(blockedWhen(now - 72 * 3_600_000, now)).toBe("Blocked 3 days ago");
  });
  it("does not invent a time it does not have", () => {
    expect(blockedWhen(0, now)).toBe("Blocked");
  });
});

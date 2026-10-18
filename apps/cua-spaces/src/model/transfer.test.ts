// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  formatMegabytes,
  reduceTransfer,
  transferProgress,
  transferSizeLabel,
  transferTitle,
  type TransferOverlayState,
} from "./transfer";

describe("transfer overlay state", () => {
  it("start opens the active overlay with the app name", () => {
    const state = reduceTransfer(null, { status: "start", appName: "Google Chrome" });
    expect(state).toEqual({ phase: "active", appName: "Google Chrome" });
  });

  it("done clears the overlay", () => {
    const active: TransferOverlayState = { phase: "active", appName: "Chrome" };
    expect(reduceTransfer(active, { status: "done" })).toBeNull();
  });

  it("a cancel is a done clear, even from the error phase", () => {
    // Cancel from the error overlay is just a terminal "done" that dismisses it.
    const failed: TransferOverlayState = { phase: "error", appName: "Chrome", message: "x" };
    expect(reduceTransfer(failed, { status: "done" })).toBeNull();
  });

  it("error carries the message and keeps the app name", () => {
    const active: TransferOverlayState = { phase: "active", appName: "Chrome" };
    const state = reduceTransfer(active, { status: "error", message: "network down" });
    expect(state).toEqual({ phase: "error", appName: "Chrome", message: "network down" });
  });

  it("a retry (start after error) returns to active and preserves the app", () => {
    const failed: TransferOverlayState = { phase: "error", appName: "Chrome", message: "x" };
    expect(reduceTransfer(failed, { status: "start" })).toEqual({ phase: "active", appName: "Chrome" });
  });

  it("titles reflect the phase", () => {
    expect(transferTitle({ phase: "active", appName: "Chrome" })).toBe("Teleporting Chrome…");
    expect(transferTitle({ phase: "error", appName: "Chrome" })).toBe("Could not teleport Chrome");
    expect(transferTitle({ phase: "active", appName: "" })).toBe("Teleporting app…");
  });

  it("progress updates the byte counters and carries the app name forward", () => {
    const active: TransferOverlayState = { phase: "active", appName: "Chrome" };
    const state = reduceTransfer(active, {
      status: "progress",
      sentBytes: 12_900_000,
      totalBytes: 131_500_000,
    });
    expect(state).toEqual({
      phase: "active",
      appName: "Chrome",
      sentBytes: 12_900_000,
      totalBytes: 131_500_000,
    });
  });

  it("a retry (start) drops stale byte counters", () => {
    const mid: TransferOverlayState = {
      phase: "active",
      appName: "Chrome",
      sentBytes: 50,
      totalBytes: 100,
    };
    expect(reduceTransfer(mid, { status: "start" })).toEqual({ phase: "active", appName: "Chrome" });
  });
});

describe("transfer progress formatting", () => {
  it("formats megabytes with one decimal", () => {
    expect(formatMegabytes(131.5 * 1024 * 1024)).toBe("131.5 MB");
    expect(formatMegabytes(0)).toBe("0.0 MB");
  });

  it("computes a clamped determinate fraction only when totals are known", () => {
    expect(transferProgress({ phase: "active", appName: "x" })).toBeNull();
    expect(
      transferProgress({ phase: "active", appName: "x", sentBytes: 0, totalBytes: 0 }),
    ).toBeNull();
    expect(
      transferProgress({ phase: "active", appName: "x", sentBytes: 50, totalBytes: 200 }),
    ).toBeCloseTo(0.25);
    // Over-run is clamped to 1.
    expect(
      transferProgress({ phase: "active", appName: "x", sentBytes: 300, totalBytes: 200 }),
    ).toBe(1);
  });

  it("builds the '{x} MB / {y} MB' label only when totals are known", () => {
    expect(transferSizeLabel({ phase: "active", appName: "x" })).toBeNull();
    expect(
      transferSizeLabel({
        phase: "active",
        appName: "x",
        sentBytes: 12.3 * 1024 * 1024,
        totalBytes: 131.5 * 1024 * 1024,
      }),
    ).toBe("12.3 MB / 131.5 MB");
  });
});

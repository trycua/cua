// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { decideClipboard, offersAlways, parseClipboardMode, type ClipboardDirection, type ClipboardMode, type ClipboardOrigin } from "./clipboardGate";

const cases: Array<[ClipboardDirection, ClipboardOrigin]> = [
  ["to-guest", "paste"],
  ["to-host", "copy"],
  ["to-host", "guest"],
];

function decide(mode: ClipboardMode, viewing: boolean, direction: ClipboardDirection, origin: ClipboardOrigin, always = false) {
  return decideClipboard({ mode, viewing, direction, origin, always });
}

describe("clipboard mode parameter", () => {
  it("keeps the old two-way sync when the link says nothing", () => {
    expect(parseClipboardMode(null)).toBe("on");
    expect(parseClipboardMode(undefined)).toBe("on");
    expect(parseClipboardMode("")).toBe("on");
  });
  it("takes a word it does not know as the gated mode", () => {
    expect(parseClipboardMode("something-else")).toBe("view");
    expect(parseClipboardMode("view")).toBe("view");
    expect(parseClipboardMode("viewing")).toBe("view");
  });
  it("keeps the old behaviour and the off switch reachable", () => {
    expect(parseClipboardMode("on")).toBe("on");
    expect(parseClipboardMode("off")).toBe("off");
    expect(parseClipboardMode("0")).toBe("off");
    expect(parseClipboardMode("ask")).toBe("ask");
  });
});

describe("clipboard gate", () => {
  it("moves nothing while the tab is not focused and shown", () => {
    for (const mode of ["view", "ask"] as const)
      for (const [direction, origin] of cases) expect(decide(mode, false, direction, origin, true)).toBe("deny");
  });
  it("is off when off", () => {
    for (const [direction, origin] of cases) expect(decide("off", true, direction, origin, true)).toBe("deny");
  });
  it("lets the user's paste and copy through while viewing", () => {
    expect(decide("view", true, "to-guest", "paste")).toBe("allow");
    expect(decide("view", true, "to-host", "copy")).toBe("allow");
  });
  it("asks about a change the Space makes on its own, unless always-while-viewing", () => {
    expect(decide("view", true, "to-host", "guest")).toBe("prompt");
    expect(decide("view", true, "to-host", "guest", true)).toBe("allow");
  });
  it("asks every time in ask mode", () => {
    for (const [direction, origin] of cases) expect(decide("ask", true, direction, origin, true)).toBe("prompt");
    expect(offersAlways("ask")).toBe(false);
    expect(offersAlways("view")).toBe(true);
  });
  it("never pushes this computer's clipboard in the background or the wrong way", () => {
    expect(decide("view", true, "to-guest", "guest")).toBe("deny");
    expect(decide("view", true, "to-guest", "copy")).toBe("deny");
    expect(decide("view", true, "to-host", "paste")).toBe("deny");
  });
  it("on is the old two-way sync", () => {
    for (const [direction, origin] of cases) expect(decide("on", false, direction, origin)).toBe("allow");
  });
});

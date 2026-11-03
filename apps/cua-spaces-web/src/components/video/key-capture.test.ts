// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { KeyCapture, type KeyCaptureEvent } from "./key-capture";

type Mods = { meta?: boolean; shift?: boolean; alt?: boolean; ctrl?: boolean };
const ev = (type: "keydown" | "keyup", key: string, m: Mods = {}): KeyCaptureEvent => ({
  type,
  key,
  metaKey: m.meta ?? false,
  shiftKey: m.shift ?? false,
  altKey: m.alt ?? false,
  ctrlKey: m.ctrl ?? false,
});

// The SwiftUI app's KeyCaptureTests (libs/spaces-app-swift), on browser key events.
describe("KeyCapture", () => {
  it("sends Command chords to the Space while the viewer has the keyboard", () => {
    const c = new KeyCapture();
    expect(c.route(ev("keydown", "2", { meta: true }), true)).toBe("guest");
    expect(c.route(ev("keydown", "c", { meta: true, shift: true }), true)).toBe("guest");
    expect(c.route(ev("keydown", "c", { meta: true, ctrl: true }), true)).toBe("guest");
    expect(c.route(ev("keydown", "Escape", { meta: true }), true)).toBe("guest");
    // Plain keys and Control chords take the viewer's own handling.
    expect(c.route(ev("keydown", "a"), true)).toBe("app");
    expect(c.route(ev("keydown", "c", { ctrl: true }), true)).toBe("app");
    // Without the keyboard the app keeps its shortcuts.
    expect(c.route(ev("keydown", "c", { meta: true }), false)).toBe("app");
  });

  it("hands the keyboard back on Control+Option pressed and released alone, not on a Control+Option chord", () => {
    const c = new KeyCapture();
    expect(c.route(ev("keydown", "Control", { ctrl: true }), true)).toBe("app");
    expect(c.route(ev("keydown", "Alt", { ctrl: true, alt: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Control", { alt: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Alt"), true)).toBe("release");

    expect(c.route(ev("keydown", "Alt", { ctrl: true, alt: true }), true)).toBe("app");
    expect(c.route(ev("keydown", "t", { ctrl: true, alt: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Alt", { ctrl: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Control"), true)).toBe("app");

    expect(c.route(ev("keydown", "Alt", { ctrl: true, alt: true }), true)).toBe("app");
    expect(c.route(ev("keydown", "Shift", { ctrl: true, alt: true, shift: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Control", { alt: true, shift: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Alt", { shift: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Shift"), true), "another modifier joined").toBe("app");
  });

  it("forgets a half-typed release chord once the viewer loses the keyboard", () => {
    const c = new KeyCapture();
    expect(c.route(ev("keydown", "Alt", { ctrl: true, alt: true }), true)).toBe("app");
    expect(c.route(ev("keyup", "Alt", { ctrl: true }), false)).toBe("app");
    expect(c.route(ev("keyup", "Control"), true)).toBe("app");
  });
});

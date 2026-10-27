// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { acceleratorOf, type KeyPress } from "./shortcut-keys";

const press = (over: Partial<KeyPress>): KeyPress => ({ key: "", code: "", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...over });

describe("acceleratorOf", () => {
  it("builds the default shortcut", () => {
    expect(acceleratorOf(press({ key: " ", code: "Space", metaKey: true, shiftKey: true }))).toBe("Shift+Command+Space");
  });
  it("names letters and digits by their key, not the typed character", () => {
    expect(acceleratorOf(press({ key: "å", code: "KeyA", altKey: true }))).toBe("Alt+A");
    expect(acceleratorOf(press({ key: "1", code: "Digit1", ctrlKey: true }))).toBe("Control+1");
  });
  it("needs Command, Control or Option: Shift alone does not make a shortcut", () => {
    expect(acceleratorOf(press({ key: "A", code: "KeyA", shiftKey: true }))).toBeNull();
    expect(acceleratorOf(press({ key: "a", code: "KeyA" }))).toBeNull();
  });
  it("waits while only a modifier is down", () => {
    expect(acceleratorOf(press({ key: "Meta", code: "MetaLeft", metaKey: true }))).toBeNull();
  });
});

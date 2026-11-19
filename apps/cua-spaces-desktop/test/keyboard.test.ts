// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { KEYBOARD_ATTRIBUTE, applyKeyboard, viewerHasKeyboard } from "../src/keyboard";

const web = path.join(__dirname, "../../cua-spaces-web/src");

describe("the viewer's keyboard (the SwiftUI app's KeyCapture)", () => {
  const el = (attrs: string[]) => ({ hasAttribute: (a: string) => attrs.includes(a) });

  it("sees a viewer with the keyboard only when its canvas has focus", () => {
    expect(viewerHasKeyboard(el([KEYBOARD_ATTRIBUTE]))).toBe(true);
    expect(viewerHasKeyboard(el(["data-webcodecs"]))).toBe(false);
    expect(viewerHasKeyboard(null)).toBe(false);
    expect(viewerHasKeyboard(undefined)).toBe(false);
  });

  it("turns the menu shortcuts off while a viewer has the keyboard, on again otherwise", () => {
    const calls: boolean[] = [];
    const contents = { setIgnoreMenuShortcuts: (ignore: boolean) => void calls.push(ignore) };
    applyKeyboard(contents, true);
    applyKeyboard(contents, false);
    applyKeyboard(contents, "yes");
    applyKeyboard(contents, undefined);
    expect(calls).toEqual([true, false, false, false]);
  });

  it("matches the marker the web UI puts on the viewer's canvas", () => {
    const src = readFileSync(path.join(web, "components/video/webcodecs-slots.ts"), "utf8");
    expect(src).toContain(`"${KEYBOARD_ATTRIBUTE}"`);
  });
});

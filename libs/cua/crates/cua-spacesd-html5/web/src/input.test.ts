// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { InteractiveInputEvent } from "./core/mediaWire";
import { chord, InputController, mapBeforeInput, mapKey, shortcutKey, type KeyLike } from "./input";

const key = (over: Partial<KeyLike>): KeyLike => ({
  key: "a",
  code: "KeyA",
  shiftKey: false,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  repeat: false,
  ...over,
});
const opts = { metaAsControl: false };

describe("keyboard mapping", () => {
  it("leaves printable text to the input event", () => {
    expect(mapKey(key({}), "down", opts)).toBeNull();
    expect(mapKey(key({ key: "é", code: "Digit2" }), "down", opts)).toBeNull();
  });

  it("sends named keys and modifiers", () => {
    expect(mapKey(key({ key: "Enter", code: "Enter" }), "down", opts)).toEqual({ kind: "key", key: "enter", state: "down", modifiers: [], repeat: false });
    expect(mapKey(key({ key: "ArrowLeft", code: "ArrowLeft", shiftKey: true }), "down", opts)).toMatchObject({ key: "arrowleft", modifiers: ["shift"] });
    expect(mapKey(key({ key: "F5", code: "F5" }), "up", opts)).toMatchObject({ key: "f5", state: "up" });
    expect(mapKey(key({ key: "Control", code: "ControlLeft", ctrlKey: true }), "down", opts)).toMatchObject({ key: "control", modifiers: [] });
  });

  it("takes shortcut letters from the physical key (any layout)", () => {
    // Ctrl+C on a Russian layout produces "с" (Cyrillic).
    expect(mapKey(key({ key: "с", code: "KeyC", ctrlKey: true }), "down", opts)).toMatchObject({ key: "c", modifiers: ["control"] });
    // AZERTY: the key labelled A is at code KeyQ; the shortcut follows the letter produced? No: physical.
    expect(shortcutKey(key({ key: "a", code: "KeyQ" }))).toBe("q");
    expect(shortcutKey(key({ key: "&", code: "Digit1" }))).toBe("1");
  });

  it("maps Cmd to Ctrl for non-Mac guests when asked", () => {
    const mac = { metaAsControl: true };
    expect(mapKey(key({ key: "v", code: "KeyV", metaKey: true }), "down", mac)).toMatchObject({ key: "v", modifiers: ["control"] });
    expect(mapKey(key({ key: "Meta", code: "MetaLeft", metaKey: true }), "down", mac)).toMatchObject({ key: "control" });
  });

  it("sends the right Cmd held alone as Super, the left as Ctrl", () => {
    const surface = document.createElement("div");
    const sent: InteractiveInputEvent[] = [];
    const input = new InputController({ surface, screen: () => surface, emit: (e) => sent.push(e), keyOptions: () => ({ metaAsControl: true }) });
    const press = (type: string, init: KeyboardEventInit) => input.textarea.dispatchEvent(new KeyboardEvent(type, { cancelable: true, ...init }));
    press("keydown", { key: "Meta", code: "MetaRight", metaKey: true });
    press("keydown", { key: "q", code: "KeyQ", metaKey: true });
    press("keyup", { key: "q", code: "KeyQ", metaKey: true });
    press("keyup", { key: "Meta", code: "MetaRight" });
    press("keydown", { key: "Meta", code: "MetaLeft", metaKey: true });
    press("keydown", { key: "w", code: "KeyW", metaKey: true });
    press("keyup", { key: "w", code: "KeyW", metaKey: true });
    press("keyup", { key: "Meta", code: "MetaLeft" });
    input.dispose();
    expect(sent.map((e) => (e.kind === "key" ? `${[...e.modifiers, e.key].join("+")}:${e.state}` : e.kind))).toEqual([
      "meta:down",
      "command+q:down",
      "command+q:up",
      "meta:up",
      "control:down",
      "control+w:down",
      "control+w:up",
      "control:up",
    ]);
  });

  it("treats AltGr and IME composition as text", () => {
    const altGr = key({ key: "@", code: "KeyQ", ctrlKey: true, altKey: true, getModifierState: (m) => m === "AltGraph" });
    expect(mapKey(altGr, "down", opts)).toBeNull();
    expect(mapKey(key({ key: "Process", keyCode: 229 }), "down", opts)).toBeNull();
    expect(mapKey(key({ key: "Dead", code: "Quote" }), "down", opts)).toBeNull();
  });

  it("maps soft keyboard input", () => {
    expect(mapBeforeInput("insertText", "hé")).toEqual([{ kind: "text_commit", text: "hé" }]);
    expect(mapBeforeInput("insertLineBreak", null).map((e) => (e.kind === "key" ? `${e.key}:${e.state}` : ""))).toEqual(["enter:down", "enter:up"]);
    expect(mapBeforeInput("deleteContentBackward", null)[0]).toMatchObject({ key: "backspace" });
    expect(mapBeforeInput("formatBold", null)).toEqual([]);
  });

  it("builds chords that release in reverse", () => {
    const events = chord(["control", "alt", "delete"]).map((e) => (e.kind === "key" ? `${e.key}:${e.state}` : ""));
    expect(events).toEqual(["control:down", "alt:down", "delete:down", "delete:up", "alt:up", "control:up"]);
  });
});

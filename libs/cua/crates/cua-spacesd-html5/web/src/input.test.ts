// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { chord, InputController, mapBeforeInput, mapKey, shortcutKey, type KeyLike } from "./input";
import type { InteractiveInputEvent } from "./core/mediaWire";

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

  it("keeps the right Cmd held alone as Super for non-Mac guests", () => {
    const sent: InteractiveInputEvent[] = [];
    const surface = document.createElement("div");
    const controller = new InputController({
      surface,
      screen: () => surface,
      emit: (event) => sent.push(event),
      keyOptions: () => ({ metaAsControl: true }),
    });
    const press = (type: string, init: KeyboardEventInit) => controller.textarea.dispatchEvent(new KeyboardEvent(type, init));
    press("keydown", { key: "Meta", code: "MetaRight", metaKey: true });
    press("keydown", { key: "w", code: "KeyW", metaKey: true });
    press("keyup", { key: "w", code: "KeyW", metaKey: true });
    press("keydown", { key: "Meta", code: "MetaLeft", metaKey: true });
    press("keydown", { key: "c", code: "KeyC", metaKey: true });
    controller.textarea.dispatchEvent(new FocusEvent("blur"));
    press("keyup", { key: "Meta", code: "MetaLeft" });
    press("keyup", { key: "Meta", code: "MetaRight" });
    controller.dispose();
    expect(sent.map((e) => (e.kind === "key" ? [e.key, e.state, e.modifiers] : e.kind))).toEqual([
      ["meta", "down", []],
      ["w", "down", ["command"]],
      ["w", "up", ["command"]],
      ["meta", "up", []],
      ["control", "down", []],
      ["c", "down", ["control"]],
      // Focus loss releases everything; the late key-ups send nothing more.
      ["c", "up", ["control"]],
      ["control", "up", []],
    ]);
  });

  it("sends keys pressed during a paste after its chord", async () => {
    const sent: InteractiveInputEvent[] = [];
    let landed = () => {};
    let pastes = 0;
    const surface = document.createElement("div");
    const controller = new InputController({
      surface,
      screen: () => surface,
      emit: (event) => sent.push(event),
      keyOptions: () => ({ metaAsControl: true }),
      onPaste: () => {
        pastes++;
        return new Promise<void>((resolve) => (landed = resolve));
      },
    });
    const press = (type: string, init: KeyboardEventInit) =>
      controller.textarea.dispatchEvent(new KeyboardEvent(type, { ...init, cancelable: true }));
    press("keydown", { key: "Meta", code: "MetaRight", metaKey: true });
    press("keydown", { key: "Meta", code: "MetaLeft", metaKey: true });
    // Not cancelled: the browser fires `paste`, and the chord waits for the clipboard.
    expect(press("keydown", { key: "v", code: "KeyV", metaKey: true })).toBe(true);
    controller.textarea.dispatchEvent(new Event("paste"));
    press("keyup", { key: "v", code: "KeyV", metaKey: true });
    // A second paste meanwhile waits for the same clipboard, whole: Chromium
    // may send no key-up for a key pressed with ⌘ held.
    expect(press("keydown", { key: "v", code: "KeyV", metaKey: true })).toBe(false);
    press("keyup", { key: "Meta", code: "MetaLeft", metaKey: true });
    expect(sent.map((e) => (e.kind === "key" ? `${e.key} ${e.state}` : e.kind))).toEqual(["meta down", "meta up", "control down"]);
    landed();
    await new Promise((resolve) => setTimeout(resolve, 0));
    controller.dispose();
    expect(pastes).toBe(1);
    expect(sent.map((e) => (e.kind === "key" ? [e.key, e.state, e.modifiers] : e.kind)).slice(3)).toEqual([
      ["v", "down", ["control"]],
      ["v", "up", ["control"]],
      ["v", "down", ["control"]],
      ["v", "up", ["control"]],
      ["control", "up", []],
      ["meta", "down", []],
      ["meta", "up", []],
    ]);
  });

  it("sends a Super chord to the guest, not through the host clipboard", () => {
    const sent: InteractiveInputEvent[] = [];
    let pastes = 0;
    let copies = 0;
    const surface = document.createElement("div");
    const controller = new InputController({
      surface,
      screen: () => surface,
      emit: (event) => sent.push(event),
      keyOptions: () => ({ metaAsControl: true }),
      onPaste: () => void pastes++,
      onCopyShortcut: () => void copies++,
    });
    const press = (type: string, init: KeyboardEventInit) =>
      controller.textarea.dispatchEvent(new KeyboardEvent(type, { ...init, cancelable: true }));
    press("keydown", { key: "Meta", code: "MetaRight", metaKey: true });
    // Cancelled, so the browser fires no `paste` for the host clipboard.
    expect(press("keydown", { key: "v", code: "KeyV", metaKey: true })).toBe(false);
    press("keyup", { key: "v", code: "KeyV", metaKey: true });
    press("keydown", { key: "c", code: "KeyC", metaKey: true });
    press("keyup", { key: "c", code: "KeyC", metaKey: true });
    press("keyup", { key: "Meta", code: "MetaRight" });
    controller.dispose();
    expect(pastes).toBe(0);
    expect(copies).toBe(0);
    expect(sent.map((e) => (e.kind === "key" ? [e.key, e.state, e.modifiers] : e.kind))).toEqual([
      ["meta", "down", []],
      ["v", "down", ["command"]],
      ["v", "up", ["command"]],
      ["c", "down", ["command"]],
      ["c", "up", ["command"]],
      ["meta", "up", []],
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

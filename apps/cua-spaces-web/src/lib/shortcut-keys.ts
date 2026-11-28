// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Turns the keys a person presses into the shortcut the Mac app registers
 * (an Electron accelerator such as `Command+Shift+Space`). A shortcut needs
 * at least one of Command, Control or Option, so a bare letter never grabs
 * the keyboard from every app.
 */

export type KeyPress = Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">;

const NAMED: Record<string, string> = {
  Space: "Space",
  Enter: "Return",
  Tab: "Tab",
  Backspace: "Backspace",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
};

const MODIFIER_KEYS = new Set(["Meta", "Control", "Alt", "Shift"]);

/** The key's name for an accelerator, or null for a modifier alone or a key we don't take. */
function keyName(e: KeyPress): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const named = NAMED[e.code];
  if (named) return named;
  if (/^Key[A-Z]$/.test(e.code)) return e.code.slice(3);
  if (/^Digit\d$/.test(e.code)) return e.code.slice(5);
  if (/^F([1-9]|1\d|2[0-4])$/.test(e.code)) return e.code;
  return null;
}

/** The accelerator for a key press, or null while it is not a complete shortcut yet. */
export function acceleratorOf(e: KeyPress): string | null {
  const key = keyName(e);
  if (!key || !(e.metaKey || e.ctrlKey || e.altKey)) return null;
  const parts = [e.ctrlKey && "Control", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Command", key];
  return parts.filter(Boolean).join("+");
}

const GLYPHS: Record<string, string> = {
  Control: "\u2303",
  Alt: "\u2325",
  Shift: "\u21e7",
  Command: "\u2318",
  Return: "\u21a9",
  Backspace: "\u232b",
  Up: "\u2191",
  Down: "\u2193",
  Left: "\u2190",
  Right: "\u2192",
};

/** `Command+Shift+Space` as the Mac writes it: \u2318\u21e7Space. */
export function shortcutLabel(accelerator: string): string {
  return accelerator
    .split("+")
    .map((p) => GLYPHS[p] ?? p)
    .join("");
}

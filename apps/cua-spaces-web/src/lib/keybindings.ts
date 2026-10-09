// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import defaults from "./keybindings.json";
import { isMacPlatform } from "./utils";

export type KeybindingCommand =
  | "commandPalette.toggle"
  | "sidebar.toggle"
  | "nav.spaces"
  | "nav.machines"
  | "nav.agents"
  | "nav.keyvault"
  | "nav.settings"
  | "theme.cycle";

export interface Keybinding {
  command: KeybindingCommand;
  /** "mod+k", "mod+shift+l". `mod` is Cmd on macOS and Ctrl elsewhere. */
  key: string;
}

export interface Shortcut {
  key: string;
  mod: boolean;
  shift: boolean;
  alt: boolean;
}

export const DEFAULT_KEYBINDINGS = defaults as Keybinding[];

export function parseShortcut(spec: string): Shortcut {
  const parts = spec.toLowerCase().split("+").map((p) => p.trim());
  const key = parts.pop() ?? "";
  return { key, mod: parts.includes("mod"), shift: parts.includes("shift"), alt: parts.includes("alt") };
}

interface KeyEventLike {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export function matchesShortcut(event: KeyEventLike, spec: string, mac = isMacPlatform()): boolean {
  const s = parseShortcut(spec);
  const mod = mac ? event.metaKey : event.ctrlKey;
  const otherMod = mac ? event.ctrlKey : event.metaKey;
  return (
    event.key.toLowerCase() === s.key &&
    mod === s.mod &&
    !otherMod &&
    event.shiftKey === s.shift &&
    event.altKey === s.alt
  );
}

export function resolveCommand(
  event: KeyEventLike,
  bindings: readonly Keybinding[] = DEFAULT_KEYBINDINGS,
  mac = isMacPlatform(),
): KeybindingCommand | null {
  return bindings.find((b) => matchesShortcut(event, b.key, mac))?.command ?? null;
}

/** Display tokens for a shortcut: ["⌘", "K"] on macOS, ["Ctrl", "K"] elsewhere. */
export function shortcutLabel(spec: string, mac = isMacPlatform()): string[] {
  const s = parseShortcut(spec);
  const out: string[] = [];
  if (s.mod) out.push(mac ? "⌘" : "Ctrl");
  if (s.alt) out.push(mac ? "⌥" : "Alt");
  if (s.shift) out.push(mac ? "⇧" : "Shift");
  out.push(s.key.length === 1 ? s.key.toUpperCase() : s.key);
  return out;
}

export function bindingFor(command: KeybindingCommand, bindings: readonly Keybinding[] = DEFAULT_KEYBINDINGS) {
  return bindings.find((b) => b.command === command)?.key;
}

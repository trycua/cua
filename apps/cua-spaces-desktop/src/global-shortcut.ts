// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The global shortcut (macOS, as the SwiftUI app's): ⌘⇧Space (or the keys
// chosen in Settings, General) brings the main window to the front from any
// app, through Electron's globalShortcut.
// Another app may already hold the keys; then the registration fails, and
// Settings says so and tries again each time it opens (`ensure`).
import { app, globalShortcut } from "electron";

export const DEFAULT_ACCELERATOR = "Command+Shift+Space";

export interface GlobalShortcutState {
  label: string;
  registered: boolean;
}

const SYMBOL: Record<string, string> = { Control: "\u2303", Alt: "\u2325", Shift: "\u21e7", Command: "\u2318", Return: "\u21a9", Up: "\u2191", Down: "\u2193", Left: "\u2190", Right: "\u2192" };

/** `Command+Shift+Space` as the Mac writes it: \u2318\u21e7Space. */
export function shortcutLabel(accelerator: string): string {
  return accelerator
    .split("+")
    .map((p) => SYMBOL[p] ?? p)
    .join("");
}

/** A shortcut Settings may set: at least one of Command, Control or Option, then one key (a letter, a digit, F1 to F24, Space or an arrow). */
const ACCELERATOR = /^(?:(?:Control|Alt|Shift|Command)\+)*(?:Control|Alt|Command)\+(?:(?:Control|Alt|Shift|Command)\+)*(?:[A-Z0-9]|F(?:[1-9]|1\d|2[0-4])|Space|Return|Tab|Backspace|Up|Down|Left|Right)$/;

export const isAccelerator = (value: string): boolean => ACCELERATOR.test(value);

export class GlobalShortcut {
  private registered = false;

  constructor(
    private readonly onPress: () => void,
    private readonly register: (accelerator: string, callback: () => void) => boolean = (a, cb) => globalShortcut.register(a, cb),
    private accelerator: string = DEFAULT_ACCELERATOR,
    private readonly unregister: (accelerator: string) => void = (a) => globalShortcut.unregister(a),
    private readonly save: (accelerator: string) => void = () => {},
  ) {}

  /** Registers the keys unless they already are; what the system answered. */
  ensure(): GlobalShortcutState {
    if (!this.registered) this.registered = this.register(this.accelerator, this.onPress);
    return { label: shortcutLabel(this.accelerator), registered: this.registered };
  }

  /**
   * Moves the shortcut to `next`. The new keys are tried first: when another
   * app holds them the old ones stay, and this throws why.
   */
  change(next: string): GlobalShortcutState {
    if (!isAccelerator(next)) throw new Error(`Not a shortcut Cua Spaces can use: ${next}`);
    if (next === this.accelerator) return this.ensure();
    if (!this.register(next, this.onPress)) throw new Error(`Another app is using ${shortcutLabel(next)}. Pick other keys.`);
    if (this.registered) this.unregister(this.accelerator);
    this.accelerator = next;
    this.registered = true;
    this.save(next);
    return { label: shortcutLabel(next), registered: true };
  }
}

/** The shortcut on macOS (the Windows and Linux shells have none), registered
 * once the app is ready and released when it quits. `saved` is the keys the
 * person chose last time (none: the default). */
export function installGlobalShortcut(
  platform: NodeJS.Platform,
  onPress: () => void,
  saved: { read(): string | undefined; write(accelerator: string): void } = { read: () => undefined, write: () => {} },
): GlobalShortcut | null {
  if (platform !== "darwin") return null;
  const stored = saved.read();
  const shortcut = new GlobalShortcut(onPress, undefined, stored && isAccelerator(stored) ? stored : DEFAULT_ACCELERATOR, undefined, saved.write);
  shortcut.ensure();
  app.on("will-quit", () => globalShortcut.unregisterAll());
  return shortcut;
}

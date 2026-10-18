// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The global shortcut (macOS, as the SwiftUI app's): ⌘⇧Space brings the
// main window to the front from any app, through Electron's globalShortcut.
// Another app may already hold the keys; then the registration fails, and
// Settings says so and tries again each time it opens (`ensure`).
import { app, globalShortcut } from "electron";

export const GLOBAL_SHORTCUT = { label: "⌘⇧Space", accelerator: "Command+Shift+Space" } as const;

export interface GlobalShortcutState {
  label: string;
  registered: boolean;
}

export class GlobalShortcut {
  private registered = false;

  constructor(
    private readonly onPress: () => void,
    private readonly register: (accelerator: string, callback: () => void) => boolean = (a, cb) => globalShortcut.register(a, cb),
  ) {}

  /** Registers the keys unless they already are; what the system answered. */
  ensure(): GlobalShortcutState {
    if (!this.registered) this.registered = this.register(GLOBAL_SHORTCUT.accelerator, this.onPress);
    return { label: GLOBAL_SHORTCUT.label, registered: this.registered };
  }
}

/** The shortcut on macOS (the Windows and Linux shells have none), registered
 * once the app is ready and released when it quits. */
export function installGlobalShortcut(platform: NodeJS.Platform, onPress: () => void): GlobalShortcut | null {
  if (platform !== "darwin") return null;
  const shortcut = new GlobalShortcut(onPress);
  shortcut.ensure();
  app.on("will-quit", () => globalShortcut.unregisterAll());
  return shortcut;
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The system the app runs on, for the words that name it ("This Mac",
 * "This PC", "This computer") and its architecture.
 */

import type { HostWindow } from "./detect";

export type HostOs = "macos" | "windows" | "linux";

/** Maps a platform word (Node's `darwin`/`win32`/`linux`, or the browser's
 * `navigator.platform`: `MacIntel`, `Win32`, `Linux x86_64`) to a system.
 * Unknown or empty words are macOS, so the Mac's words stay the default. */
export function osOfPlatform(platform: string | undefined | null): HostOs {
  const p = (platform ?? "").toLowerCase();
  if (p.startsWith("win")) return "windows";
  if (p.startsWith("linux") || p.includes("x11")) return "linux";
  return "macos";
}

/**
 * The system the app runs on: what the Electron shell reports
 * (`cuaDesktop.platform`), else the browser's platform. The SwiftUI and
 * Tauri hosts run on a Mac.
 */
export function hostOs(win: HostWindow | undefined = globalThis.window as HostWindow | undefined): HostOs {
  const reported = win?.cuaDesktop?.platform;
  if (reported) return osOfPlatform(reported);
  return osOfPlatform(typeof navigator === "undefined" ? "" : navigator.platform);
}

/** What the app calls the computer it runs on. */
export const THIS_MACHINE: Record<HostOs, string> = { macos: "This Mac", windows: "This PC", linux: "This computer" };

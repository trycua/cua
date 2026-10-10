// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { app } from "electron";
import { readFileSync, writeFileSync, renameSync, mkdirSync } from "node:fs";
import * as path from "node:path";
import type { ThemeSource } from "./channels";

export interface WindowBounds {
  x?: number;
  y?: number;
  width: number;
  height: number;
  maximized?: boolean;
}

/** The one-time takeover of the Swift app's state (migrate-swift.ts). */
export interface SwiftMigration {
  at: string;
  /** The Swift app had left state on this Mac. */
  found: boolean;
  /** The Swift app's files copied (settings.json, onboarding.json). */
  copied: string[];
  /** The version the Swift app last ran as. */
  from?: string;
  /** Sparkle's "check automatically" / "download and install" when the user had changed them. */
  sparkleAutomaticChecks?: boolean;
  sparkleAutomaticDownloads?: boolean;
}

export interface Settings {
  windowBounds?: WindowBounds;
  themeSource?: ThemeSource;
  /** Automatic update checks: opt-in; see updater.ts. */
  autoUpdate?: boolean;
  /** Download and install updates on their own (Settings → About). */
  autoInstall?: boolean;
  /** The last update check (ms). */
  lastUpdateCheck?: number;
  /** Update channel; defaults to the one the build was cut for (see updater.ts). */
  updateChannel?: "stable" | "beta";
  /** Where the last picture-in-picture panel was left (pip.ts). */
  pipBounds?: { x: number; y: number; width: number; height: number };
  /** Where the last Space viewer was left, and its content's width (viewer.ts). */
  viewerBounds?: { x: number; y: number; width: number };
  /** Present once the Swift app's state was taken over (macOS). */
  swiftMigration?: SwiftMigration;
}

const file = () => path.join(app.getPath("userData"), "settings.json");
let cache: Settings | null = null;

export function readSettings(): Settings {
  if (cache) return cache;
  try {
    cache = JSON.parse(readFileSync(file(), "utf8")) as Settings;
  } catch {
    cache = {};
  }
  return cache;
}

export function writeSettings(patch: Partial<Settings>): void {
  cache = { ...readSettings(), ...patch };
  try {
    mkdirSync(path.dirname(file()), { recursive: true });
    const tmp = `${file()}.tmp`;
    writeFileSync(tmp, JSON.stringify(cache, null, 2));
    renameSync(tmp, file());
  } catch (error) {
    console.warn("[cua-spaces] could not save settings:", error);
  }
}

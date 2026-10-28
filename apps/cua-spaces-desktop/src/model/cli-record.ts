// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { execFile } from "node:child_process";
import * as path from "node:path";

/**
 * Windows: the key the uninstaller reads (packaging/installer.nsh) to remove
 * the `cua` this app put on PATH with the app, and nothing it did not: the
 * file it copied, and its folder's PATH entry when this app added it.
 * Elsewhere the CLI stays, like the user's ~/.cua: the .deb's is a link into
 * the app that goes with it, as the SwiftUI app's into its bundle.
 */
export const WINDOWS_APP_KEY = "HKCU\\Software\\ai.cua.spaces.desktop";

/** The `reg add` calls that record an installed CLI. */
export function cliRecordArgs(target: string, addedToPath: boolean): string[][] {
  const value = (name: string, data: string) => ["add", WINDOWS_APP_KEY, "/v", name, "/t", "REG_SZ", "/d", data, "/f"];
  return [value("CliInstalled", target), ...(addedToPath ? [value("CliPathAdded", path.win32.dirname(target))] : [])];
}

const reg = (args: string[]) =>
  new Promise<void>((resolve, reject) => execFile("reg", args, { windowsHide: true }, (error) => (error ? reject(error) : resolve())));

/**
 * Windows: records a CLI the app put on PATH (after "Install" in the first
 * run's terminal step or in Settings), so the uninstaller removes it. Nothing
 * is written elsewhere, and nothing is installed unless someone asks.
 */
export async function recordCliInstall(target: string, addedToPath: boolean, platform: NodeJS.Platform = process.platform, record = reg): Promise<void> {
  if (platform !== "win32") return;
  for (const args of cliRecordArgs(target, addedToPath)) await record(args);
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Whether this machine has a web browser to open the sign-in page in. A Mac
// and Windows always do. A Linux box may not (a server, a minimal desktop):
// xdg-open then fails with a dialog of its own and the sign-in waits for a
// browser that never opens, so the app signs in with the device flow
// instead (src/model/services.ts).
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

export interface BrowserProbe {
  platform: NodeJS.Platform;
  env: NodeJS.ProcessEnv;
  /** `xdg-settings get default-web-browser` ("firefox.desktop", or "" when it names none). */
  defaultBrowser(): string;
  exists(file: string): boolean;
}

const liveProbe = (): BrowserProbe => ({
  platform: process.platform,
  env: process.env,
  defaultBrowser: () => {
    try {
      return execFileSync("xdg-settings", ["get", "default-web-browser"], { encoding: "utf8", timeout: 2_000, stdio: ["ignore", "pipe", "ignore"] }).trim();
    } catch {
      return "";
    }
  },
  exists: existsSync,
});

/** Where desktop entries live (the XDG data dirs). */
function applicationDirs(env: NodeJS.ProcessEnv): string[] {
  const home = env.XDG_DATA_HOME || path.join(env.HOME || os.homedir(), ".local/share");
  const dirs = (env.XDG_DATA_DIRS || "/usr/local/share:/usr/share").split(":").filter(Boolean);
  return [home, ...dirs, "/var/lib/snapd/desktop", "/var/lib/flatpak/exports/share"].map((d) => path.join(d, "applications"));
}

/** On Linux: `$BROWSER`, or a default browser whose desktop entry is installed. */
export function hasBrowser(probe: BrowserProbe = liveProbe()): boolean {
  if (probe.platform !== "linux") return true;
  if (probe.env.BROWSER?.trim()) return true;
  const entry = probe.defaultBrowser();
  if (!entry) return false;
  return applicationDirs(probe.env).some((dir) => probe.exists(path.join(dir, entry)));
}

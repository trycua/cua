// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The Cua Driver's words, shared by its page, the top bar and the first run. Plain words only. */

import { sinceText, type DriverStatus } from "@/bridge";

export const DRIVER_LINE = "Lets agents see and use the apps on this computer.";

const place = (s: DriverStatus) => (s.thisMachine ? "this Mac" : s.machineName);

/** "Working on this Mac." and friends: the status in one sentence. */
export function stateText(s: DriverStatus): string {
  switch (s.state) {
    case "ready":
      return `Working on ${place(s)}.`;
    case "paused":
      return "Paused. Agents can't use this Mac until you resume.";
    case "needs-permission":
      return "Needs your OK in System Settings.";
    case "starting":
      return "Starting.";
    case "stopped":
      return "Not running.";
    case "failed":
      return "Stopped working.";
  }
}

/** "Last test passed 2 minutes ago." / "Last test failed: …" / null. */
export function testText(s: DriverStatus, now = Date.now()): string | null {
  const t = s.lastTest;
  if (!t) return null;
  if (t.passed) return `Last test passed ${sinceText(t.atMs, now)}.`;
  return `Last test failed: ${t.failure ?? "no reason given"}.`;
}

/** "Version 0.38.0. Version 0.38.1 is ready: faster clicks in Chrome." */
export function versionText(s: DriverStatus): string | null {
  if (!s.version) return null;
  const update = s.update ? ` Version ${s.update.version} is ready${s.update.note ? `: ${lowerFirst(s.update.note)}` : "."}` : "";
  return `Version ${s.version}.${update}`;
}

/** The top bar's line. */
export function topBarText(s: DriverStatus): string {
  switch (s.state) {
    case "ready":
      return "Agents can use this Mac";
    case "paused":
      return "Agents paused";
    case "needs-permission":
      return "Agents can't use this Mac yet";
    case "starting":
      return "Cua Driver is starting";
    default:
      return "Cua Driver needs a look";
  }
}

const lowerFirst = (s: string) => (s ? s[0]!.toLowerCase() + s.slice(1) : s);

/** The app mark id for a coding app (`components/agents/harness-mark.tsx`). */
export const markFor = (app: string) => (app === "t3-code" ? "t3code" : app);

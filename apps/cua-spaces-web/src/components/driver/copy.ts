// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The Cua Driver page's words. Plain words only. */

import { sinceText, type DriverStatus } from "@/bridge";
import { thisComputerText } from "@/lib/host-labels";

export const driverLine = () => `Lets agents see and use the apps on ${thisComputerText()}.`;

const place = (s: DriverStatus) => (s.thisMachine ? thisComputerText() : s.machineName);

/** "Working on this Mac." and friends: the status in one sentence. */
export function stateText(s: DriverStatus): string {
  switch (s.state) {
    case "ready":
      return `Working on ${place(s)}.`;
    case "paused":
      return `Off. Agents can't use apps on ${thisComputerText()}.`;
    case "needs-permission":
      return "A permission is missing. Allow it below.";
    case "starting":
      return "Starting.";
    case "stopped":
      return "Not running.";
    case "failed":
      return "Not answering.";
  }
}

/** What the Cua Driver line in the sidebar and This Mac's Cua Driver row say when the driver needs a person. */
export interface DriverAttention {
  kind: "needs-permission" | "failed" | "stopped" | "update";
  /** One or two words beside the line. */
  word: string;
  /** The tooltip: what is wrong and what to do, in a sentence. */
  tip: string;
}

/**
 * Null while Cua Driver works, is starting, or was paused on purpose, so the
 * sidebar stays quiet. Otherwise the most urgent thing, from the same state
 * and update the page shows.
 */
export function driverAttention(s: DriverStatus): DriverAttention | null {
  switch (s.state) {
    case "needs-permission":
      return { kind: "needs-permission", word: "Needs access", tip: "Cua Driver needs a permission in System Settings. Open it to allow it." };
    case "failed":
      return { kind: "failed", word: "Not answering", tip: "Cua Driver keeps stopping. Open it to repair it." };
    case "stopped":
      return { kind: "stopped", word: "Stopped", tip: "Cua Driver isn't running. Open it to repair it." };
  }
  if (s.update) return { kind: "update", word: "Update ready", tip: `Cua Driver ${s.update.version} is ready. Open it to update.` };
  return null;
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

const lowerFirst = (s: string) => (s ? s[0]!.toLowerCase() + s.slice(1) : s);

/** The app mark id for a coding app (`components/agents/harness-mark.tsx`). */
export const markFor = (app: string) => (app === "t3-code" ? "t3code" : app);

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The words for Cua Spaces' two permissions, the same wherever they show
 * (the first run's step, its next steps, the Cua Driver page, This Mac).
 * Each names the app System Settings lists ("Cua Spaces Local" for the
 * Local build, which has its own grants), so nobody turns on the wrong one.
 */

import type { DriverPermission } from "@/bridge/ops/driver";

export interface PermissionWords {
  title: string;
  sub: string;
}

export function permissionWords(id: DriverPermission["id"], appName: string): PermissionWords {
  return id === "screen"
    ? { title: "See the screen", sub: `Screen Recording, given to ${appName}` }
    : { title: "Use the mouse and keyboard", sub: `Accessibility, given to ${appName}` };
}

export type PermissionState = NonNullable<DriverPermission["state"]>;

/** Where one permission stands, from the host's answer (an older host says only granted or not). */
export function permissionState(p: Pick<DriverPermission, "granted" | "state">): PermissionState {
  if (p.granted) return "on";
  return p.state && p.state !== "on" ? p.state : "unknown";
}

/**
 * A row's live status, beside Allow: "On" once granted; while the person is in System Settings after Allow (and
 * after the app reopens for Screen Recording), that it is being checked; otherwise what System Settings shows now.
 */
export function permissionStatus(p: Pick<DriverPermission, "granted" | "state">, waiting: boolean): { label: string; done: boolean; checking: boolean } | null {
  const state = permissionState(p);
  if (state === "on") return { label: "On", done: true, checking: false };
  if (waiting) return { label: "Waiting for the switch", done: false, checking: true };
  if (state === "off") return { label: "Off", done: false, checking: false };
  if (state === "absent") return { label: "Not in the list yet", done: false, checking: false };
  return null;
}

/** What Allow does, under the rows; once both are on, that they are. */
export function allowHint(appName: string, allGranted = false): string {
  return allGranted ? "Both are on. Agents can use apps on this Mac." : `Allow opens System Settings. Turn on ${appName} there, then come back.`;
}

/**
 * Whether Home shows its apps row on its own, outside "Now bring the rest in" (CUA-3551): on a Mac, whenever a
 * permission is missing, at every launch, whatever setup did; and, once shown, until Home is left, so a switch turned
 * on in System Settings shows as On there instead of the row vanishing.
 */
export function homeAppsRowShown(input: { os: string; available: boolean; allGranted: boolean; shownBefore: boolean }): boolean {
  if (input.os !== "macos" || !input.available) return false;
  return !input.allGranted || input.shownBefore;
}

/** The line under Home's apps row title: how many switches are still off, or that both are on. */
export function homeAppsRowLine(permissions: readonly Pick<DriverPermission, "granted">[]): string {
  const off = permissions.filter((p) => !p.granted).length;
  if (off === 0) return allowHint("", true);
  return off === 1 ? "One switch in System Settings is still off." : "Two switches in System Settings are still off.";
}

/** Local Network, on a Mac that runs macOS Spaces: its row's words, the same wherever it shows. */
export function localNetworkWords(appName: string): PermissionWords {
  return { title: "Run macOS Spaces on this Mac", sub: `Local Network, given to ${appName}` };
}

/**
 * The Local Network row's live status: On once allowed; after Allow, while macOS's question waits for an answer;
 * Off once the person said no; nothing before Allow (nothing was checked, so nothing is known).
 */
export function localNetworkStatus(input: { granted: boolean; waiting: boolean; denied: boolean }): { label: string; done: boolean; checking: boolean } | null {
  if (input.granted) return { label: "On", done: true, checking: false };
  if (input.waiting) return { label: "Waiting for your answer", done: false, checking: true };
  if (input.denied) return { label: "Off", done: false, checking: false };
  return null;
}

/** The Local Network row's button: Allow makes macOS ask; once it said no, the button opens the settings instead. */
export function localNetworkButton(denied: boolean): string {
  return denied ? "Open Settings" : "Allow";
}

/**
 * Said just before macOS asks (under the rows), so its question is no surprise; once it said no, where to turn it on.
 * Plain words: macOS's own question says "find and connect to devices on your local network".
 */
export function localNetworkNote(appName: string, denied: boolean): string {
  return denied
    ? `macOS Spaces can't start until you turn on ${appName} in System Settings, under Privacy & Security, Local Network.`
    : `macOS Spaces run on a small network inside this Mac. After Allow, macOS asks if ${appName} can find devices on your local network. Click Allow there too.`;
}

/** The first run's permissions step. */
export interface PermissionsStepView {
  title: string;
  line: string;
  /** Under the rows. */
  note: string;
  /**
   * The button. Continue (white) once everything is on. Until then it says it goes on without what is still off, so
   * nobody reads it as done (the step never blocks the run).
   */
  primary: { label: string; white: boolean };
  /** The rows still off, in the order they show: `id` is the row's `data-app-permission`. */
  missing: { id: string; title: string }[];
  /** Beside the button while something is off: what, by the rows' own titles. Pressing it shows that row. */
  stillOff: string | null;
}

/** "A", "A and B", "A, B and C". */
export function listWords(items: readonly string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** The button and the line beside it, from the rows still off. */
function stepPrimary(missing: PermissionsStepView["missing"]): Pick<PermissionsStepView, "primary" | "stillOff"> {
  if (missing.length === 0) return { primary: { label: "Continue", white: true }, stillOff: null };
  return {
    primary: { label: missing.length === 1 ? "Continue without it" : "Continue without them", white: false },
    stillOff: `Still off: ${listWords(missing.map((m) => m.title))}`,
  };
}

/**
 * `off`: the app's permissions still off (`screen`, `accessibility`); without it, both when `allGranted` is false.
 * `localNetwork`: this Mac runs macOS Spaces, so the step also asks for Local Network (`off` until allowed, `denied`
 * once macOS was told no); `none` on any other computer.
 */
export function permissionsStepView(input: {
  appName: string;
  allGranted: boolean;
  off?: readonly DriverPermission["id"][];
  localNetwork?: "none" | "off" | "denied" | "on";
}): PermissionsStepView {
  const ln = input.localNetwork ?? "none";
  const off = input.allGranted ? [] : (input.off ?? (["screen", "accessibility"] as const));
  const missing = off.map((id) => ({ id: String(id), title: permissionWords(id, input.appName).title }));
  if (ln === "none") {
    return {
      title: `Let ${input.appName} see and use apps`,
      line: "Agents can then use the apps on this Mac. Your own pointer never moves.",
      note: input.allGranted ? allowHint(input.appName, true) : `${allowHint(input.appName)} You can also do this later on the Cua Driver page.`,
      missing,
      ...stepPrimary(missing),
    };
  }
  if (ln !== "on") missing.push({ id: "local-network", title: localNetworkWords(input.appName).title });
  const done = input.allGranted && ln === "on";
  return {
    title: `Let ${input.appName} use this Mac`,
    line: "Agents can then use the apps on this Mac, and macOS Spaces can run here. Your own pointer never moves.",
    note: done
      ? "All on. Agents can use apps on this Mac, and macOS Spaces can run here."
      : ln === "on"
        ? `${allowHint(input.appName)} You can also do this later on the Cua Driver page.`
        : localNetworkNote(input.appName, ln === "denied"),
    missing,
    ...stepPrimary(missing),
  };
}

/** What the first run's "Ready" waits for: Cua Driver running, with both permissions. */
export type DriverReadiness = "ready" | "permissions" | "starting" | "driver";

/**
 * Whether the coding apps can really use Cua Driver yet. A host with no
 * permissions to give or no driver (Windows, Linux, an older app) has
 * nothing to wait for. A driver the person paused is their choice.
 */
export function driverReadiness(input: { permissions: { available: boolean; allGranted: boolean }; driver: { state: string } | null }): DriverReadiness {
  if (!input.driver || !input.permissions.available) return "ready";
  if (!input.permissions.allGranted) return "permissions";
  switch (input.driver.state) {
    case "ready":
    case "paused":
      return "ready";
    case "starting":
      return "starting";
    // A switch the driver hasn't noticed yet: the host restarts it once both are on.
    case "needs-permission":
      return "starting";
    default:
      return "driver";
  }
}

/** The words under "Almost ready" while something is missing. */
export function readinessLede(readiness: Exclude<DriverReadiness, "ready">, appName: string, problem: string | null): string {
  switch (readiness) {
    case "permissions":
      return `Turn on two switches for ${appName} in System Settings.`;
    case "starting":
      return "Cua Driver is starting. This takes a few seconds.";
    case "driver":
      return `${problem ?? "Cua Driver isn't running."} Repair starts it again.`;
  }
}

export interface RepairOutcome {
  /** `ok`: working. `permission`: a switch is off, so open its guide. `failed`: show why. */
  kind: "ok" | "permission" | "failed";
  title: string;
  /** The plain reason and the one next step. */
  description?: string;
  /** The pane to open for `permission`. */
  settingsUrl?: string | null;
}

/** What Repair and Set up tell the person once they have run, from the status they leave. */
export function repairOutcome(after: {
  state: string;
  appName?: string | null;
  permissions: { id: "screen" | "accessibility"; granted: boolean; settingsUrl: string | null }[];
  lastError: string | null;
  lastTest: { passed: boolean; failure: string | null } | null;
}): RepairOutcome {
  const app = after.appName ?? "Cua Spaces";
  const missing = after.permissions.find((p) => !p.granted);
  if (missing) {
    const words = permissionWords(missing.id, app);
    return {
      kind: "permission",
      title: "One switch is still off",
      description: `${words.title} is off. In System Settings, turn on ${app}, then come back.`,
      settingsUrl: missing.settingsUrl,
    };
  }
  if (after.state === "ready" && (!after.lastTest || after.lastTest.passed)) {
    return { kind: "ok", title: "Cua Driver is ready", description: "Start a new Claude Code or Codex session to use it." };
  }
  if (after.state === "paused") {
    return { kind: "failed", title: "Cua Driver is off", description: "Agents can't use apps on this Mac. Turn on Let agents use apps, then Repair." };
  }
  const reason = after.lastError ?? after.lastTest?.failure ?? "Cua Driver isn't answering.";
  return {
    kind: "failed",
    title: "Cua Driver still isn't working",
    description: `${reason.replace(/\.$/, "")}. Quit and reopen ${app}, then try Repair again.`,
  };
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The first launch after an update (the SwiftUI app's UpdateRefresh.swift):
// refresh what cua installed into the coding agents (`cua agents update`,
// which skips folders the user edited) with the bundled `cua`, then say so
// when it failed. The core decides (`appAboutAfterLaunch`,
// `appAboutRefreshNotice`); this runs the command. This app's own daemon is
// replaced by the daemon supervisor's `cua daemon start`.
import { execFile } from "node:child_process";
import type { Native } from "../native/load";

export interface CommandResult {
  /** The exit status; null when the process did not start or was stopped. */
  status: number | null;
  /** Standard output, then standard error. */
  output: string;
  timedOut: boolean;
}

/** Runs a command to its end (or `timeoutSeconds`). */
export type CommandRunner = (executable: string, args: string[], timeoutSeconds: number) => Promise<CommandResult>;

export const runCommand: CommandRunner = (executable, args, timeoutSeconds) =>
  new Promise((resolve) => {
    execFile(executable, args, { timeout: timeoutSeconds * 1000, killSignal: "SIGKILL", maxBuffer: 4 * 1024 * 1024, windowsHide: true }, (error, stdout, stderr) => {
      const e = error as (NodeJS.ErrnoException & { killed?: boolean; code?: unknown }) | null;
      resolve({
        status: e === null ? 0 : typeof e.code === "number" ? e.code : null,
        output: `${stdout}${stderr}`,
        timedOut: e !== null && e.killed === true,
      });
    });
  });

/** The JSON object `cua --json` printed (pretty-printed, maybe after other lines). */
export function cliJson(output: string): Record<string, unknown> | null {
  const trimmed = output.trim();
  const parse = (text: string) => {
    try {
      const v: unknown = JSON.parse(text);
      return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
    } catch {
      return null;
    }
  };
  const whole = parse(trimmed);
  if (whole) return whole;
  const start = trimmed.lastIndexOf("\n{");
  return start < 0 ? null : parse(trimmed.slice(start + 1));
}

/** The failed outcomes of `cua --json agents update` ("skill cua: why"). */
export function failures(output: string): string | null {
  const outcomes = cliJson(output)?.outcomes;
  if (!Array.isArray(outcomes)) return null;
  const failed = outcomes
    .filter((o): o is Record<string, unknown> => !!o && typeof o === "object" && (o as Record<string, unknown>).change === "failed")
    .map((o) => {
      const item = typeof o.item === "string" ? o.item : "?";
      const detail = typeof o.detail === "string" ? o.detail : "";
      return detail === "" ? item : `${item}: ${detail}`;
    });
  return failed.length ? failed.join("; ") : null;
}

/** `cua agents update`. Returns what failed, or null. */
export async function updateAgents(cua: string, run: CommandRunner = runCommand): Promise<string | null> {
  const r = await run(cua, ["--json", "agents", "update"], 120);
  if (r.timedOut) return "`cua agents update` did not finish within 2 minutes";
  if (r.status === 0) return null;
  return failures(r.output) ?? `\`cua agents update\` exited with status ${r.status ?? "none"}`;
}

/** The agents' refresh; the notice to show, or null when it worked. */
export async function refreshAfterUpdate(native: Native, cua: string, daemonError: string | null = null, run: CommandRunner = runCommand): Promise<string | null> {
  const agents = await updateAgents(cua, run);
  console.log(`[cua-spaces] refreshed after an update (agents: ${agents ?? "ok"}, daemon: ${daemonError ?? "ok"})`);
  return native.appAboutRefreshNotice({ agentsError: agents ?? undefined, daemonError: daemonError ?? undefined }) ?? null;
}

/**
 * Records this launch's version in the settings file and says whether it
 * follows an update (a fresh install and a relaunch do not). Before the
 * model loads the file: the model saves its own copy.
 */
export function recordLaunch(native: Native, settingsPath: string, version: string, build: string, onboarded: boolean): boolean {
  const settings = native.appSettingsLoad(settingsPath);
  const plan = native.appAboutAfterLaunch({ lastSeen: settings.lastSeenVersion, version, build, onboarded });
  if (plan.save !== undefined) {
    settings.lastSeenVersion = plan.save;
    try {
      native.appSettingsSave(settingsPath, settings);
    } catch (error) {
      console.warn(`[cua-spaces] could not record this launch's version: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return plan.refresh;
}

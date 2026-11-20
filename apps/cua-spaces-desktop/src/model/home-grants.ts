// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the person approved for an agent home while it runs (CUA-2630): see
// every Space, use a machine. It is written to `<cua home>/agent-home/grants.json`,
// which this machine's cua daemon reads each time the home asks
// (`children/home.rs`), so an approval takes effect at once with no restart.
// The file is outside the home Space: the agent in it can never write it.
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

export interface HomeGrantsArg {
  listAll?: boolean;
  machines?: string[];
  /** Spaces and machines @mentioned in a later request (ids and names): an @mention counts as the grant. */
  mentions?: string[];
}

/** The cua home the daemon uses (`CUA_HOME`, else `~/.cua`). */
export function cuaHomeOf(env: NodeJS.ProcessEnv): string {
  return env.CUA_HOME && env.CUA_HOME !== "" ? env.CUA_HOME : path.join(os.homedir(), ".cua");
}

/** The grants in a `spaces.setAccess` request's `home`; the wrong types are ignored. */
export function homeGrantsOf(v: unknown): HomeGrantsArg | null {
  if (!v || typeof v !== "object" || Array.isArray(v)) return null;
  const o = v as Record<string, unknown>;
  const out: HomeGrantsArg = {};
  if (typeof o.listAll === "boolean") out.listAll = o.listAll;
  if (Array.isArray(o.machines)) out.machines = [...new Set(o.machines.filter((m): m is string => typeof m === "string" && m !== ""))].sort();
  if (Array.isArray(o.mentions)) out.mentions = [...new Set(o.mentions.filter((m): m is string => typeof m === "string" && m !== ""))].sort();
  return Object.keys(out).length ? out : null;
}

/** Sets `space`'s grants in the file, keeping the other homes'. */
export async function writeHomeGrants(cuaHome: string, space: string, g: HomeGrantsArg): Promise<void> {
  const dir = path.join(cuaHome, "agent-home");
  const file = path.join(dir, "grants.json");
  let all: Record<string, { list_all?: boolean; machines?: string[]; mentions?: string[] }> = {};
  try {
    const parsed = JSON.parse(await readFile(file, "utf8")) as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) all = parsed as typeof all;
  } catch {
    // No file yet, or a broken one: start again.
  }
  const prev = all[space] ?? {};
  // Mentions only accumulate: a later request never forgets what an earlier one named.
  const mentions = [...new Set([...(prev.mentions ?? []), ...(g.mentions ?? [])])].sort();
  all[space] = { list_all: g.listAll ?? prev.list_all ?? false, machines: g.machines ?? prev.machines ?? [], ...(mentions.length ? { mentions } : {}) };
  await mkdir(dir, { recursive: true, mode: 0o700 });
  const tmp = `${file}.${process.pid}.tmp`;
  await writeFile(tmp, JSON.stringify(all), { mode: 0o600 });
  await rename(tmp, file);
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Local Spaces on Linux run in Docker (or Podman). A new user installs Docker
 * after Cua Spaces and adds themselves to the `docker` group, and the app
 * then kept saying "No local runtime found" until a reboot (CUA-1763): group
 * membership is read once, when a login session starts, and every process
 * started from that session (the app, the cua daemon that outlives it)
 * keeps the groups it started with. A logout does not help while a
 * lingering user manager (`loginctl enable-linger`) or a disconnected xrdp
 * session keeps the old processes alive.
 *
 * The standard answer is `sg docker -c <command>` (the same as `newgrp`):
 * any member of the group can start a process that has the group, without a
 * password. So the app
 *
 * - tells "in the docker group, but this session predates it" from "not in
 *   the group at all" (`getent group docker` against the groups the process
 *   really has, in `/proc/self/status`);
 * - starts itself again under `sg docker` by itself at launch, and from a
 *   "Restart Cua Spaces" button while it runs (Docker was installed while it
 *   was open); the relaunched app replaces a cua daemon that started
 *   without the group (see `DaemonSupervisor.start`);
 * - for a user who is not in the group, says the one command to run.
 *
 * Everything that touches the system goes through {@link LinuxSystem}, so
 * the decisions are tested without Linux.
 */

import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import * as os from "node:os";
import type { RuntimeFix } from "./windows-runtime";

/** What the page's fix button runs on Linux (`spaces.fixRuntime {action}`). */
export type LinuxFixAction = "restart-app";

/** Set in the relaunched app, so it never relaunches itself twice. */
export const SG_DOCKER_ENV = "CUA_SPACES_SG_DOCKER";

export interface DockerGroupFacts {
  /** The `docker` group's id (null: no such group, Docker Engine is not installed). */
  gid: number | null;
  /** The user is listed in the group (the group database, which a new session reads). */
  member: boolean;
  /** This process has the group (it started in a session that already had it, or under `sg`). */
  sessionHas: boolean;
  /** `/var/run/docker.sock` exists (Docker Engine is installed and its service was started). */
  socket: boolean;
}

/** The `docker` group's id and whether `user` is in it, from a `getent group docker` line (`docker:x:999:alice,bob`). */
export function parseGroupEntry(line: string | null, user: string): { gid: number | null; member: boolean } {
  const parts = (line ?? "").trim().split(":");
  const gid = Number.parseInt(parts[2] ?? "", 10);
  if (parts.length < 3 || !Number.isInteger(gid)) return { gid: null, member: false };
  const members = (parts[3] ?? "").split(",").map((m) => m.trim());
  return { gid, member: members.includes(user) };
}

/** Every group id a process has, from its `/proc/<pid>/status` (the Gid line and the supplementary Groups line). */
export function statusGids(status: string): number[] {
  const out = new Set<number>();
  for (const line of status.split("\n")) {
    const m = /^(Gid|Groups):\s*(.*)$/.exec(line);
    if (!m) continue;
    for (const n of m[2]!.trim().split(/\s+/)) {
      const gid = Number.parseInt(n, 10);
      if (Number.isInteger(gid)) out.add(gid);
    }
  }
  return [...out];
}

/** The user is in the group, but this process predates it. */
export function needsGroupRefresh(f: DockerGroupFacts): boolean {
  return f.gid !== null && f.member && !f.sessionHas;
}

/** The doctor's word for the container engine says the socket exists but this user may not use it. */
export function isPermissionDenied(detail: string | undefined): boolean {
  return /permission denied/i.test(detail ?? "");
}

/**
 * The reason and the fix New Space shows on Linux when the container engine
 * is there but this app cannot use it; null for anything else (not
 * installed, not running), which `noLocalRuntimeReason` words.
 */
export function linuxRuntimeAdvice(facts: DockerGroupFacts, containerDetail: string | undefined, canRelaunch: boolean, relaunched: boolean): { reason: string; fix: RuntimeFix | null } | null {
  const denied = isPermissionDenied(containerDetail) || (facts.socket && !facts.sessionHas && facts.gid !== null);
  if (!denied) return null;
  if (needsGroupRefresh(facts) && canRelaunch && !relaunched) {
    return {
      reason: "Docker is ready, and your account is in its group, but Cua Spaces started before that. Restart it to pick the group up (no logout or reboot needed).",
      fix: { action: "restart-app", label: "Restart Cua Spaces", note: "Takes a few seconds. Your Spaces keep running.", busy: false, termsUrl: null },
    };
  }
  if (facts.member) {
    // Already restarted under the group and still denied, or no `sg`: only a new login session has it.
    return {
      reason: "Docker is installed and your account is in its group, but this session doesn't have it yet. Log out of this desktop session completely and back in (or reboot), then open Cua Spaces.",
      fix: null,
    };
  }
  return {
    reason: "Docker is installed, but your account isn't allowed to use it yet. Run `sudo usermod -aG docker $USER` in a terminal, then reopen Cua Spaces.",
    fix: null,
  };
}

/** `sg docker -c 'VAR=1 exec <target> <args>'`: the command that starts this app again with the docker group. */
export function sgRelaunch(target: string, args: string[], sg = "/usr/bin/sg"): { execPath: string; args: string[] } {
  const quote = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;
  const command = `${SG_DOCKER_ENV}=1 exec ${[target, ...args].map(quote).join(" ")}`;
  return { execPath: sg, args: ["docker", "-c", command] };
}

/** The system reads and the relaunch the decisions above use. */
export interface LinuxSystem {
  user: string;
  /** `getent group docker` (null when there is no such group). */
  groupEntry(): Promise<string | null>;
  /** This process's `/proc/self/status`. */
  status(): Promise<string>;
  socketExists(): boolean;
  /** `sg` is installed. */
  hasSg(): boolean;
  /** Starts the app again under `sg docker` and quits this one. */
  relaunch(): void;
}

export class LinuxRuntime {
  private asked = false;

  constructor(
    private readonly sys: LinuxSystem,
    private readonly env: NodeJS.ProcessEnv = process.env,
  ) {}

  async facts(): Promise<DockerGroupFacts> {
    const [entry, status] = await Promise.all([this.sys.groupEntry().catch(() => null), this.sys.status().catch(() => "")]);
    const { gid, member } = parseGroupEntry(entry, this.sys.user);
    return { gid, member, sessionHas: gid !== null && statusGids(status).includes(gid), socket: this.sys.socketExists() };
  }

  /** The advice for a machine whose runtime did not answer, or null. */
  async advice(containerDetail: string | undefined): Promise<{ reason: string; fix: RuntimeFix | null } | null> {
    const facts = await this.facts();
    return linuxRuntimeAdvice(facts, containerDetail, this.sys.hasSg(), this.env[SG_DOCKER_ENV] === "1");
  }

  /** At launch: starts the app again under the group when it is needed. True when it did (this process quits). */
  async relaunchIfNeeded(): Promise<boolean> {
    if (this.env[SG_DOCKER_ENV] === "1" || !this.sys.hasSg()) return false;
    const facts = await this.facts();
    if (!facts.socket || !needsGroupRefresh(facts)) return false;
    this.relaunch();
    return true;
  }

  run(action: LinuxFixAction): void {
    if (action !== "restart-app" || this.asked) return;
    this.relaunch();
  }

  private relaunch(): void {
    this.asked = true;
    this.sys.relaunch();
  }
}

/** What to start in place of this app: `/usr/bin/sg docker -c 'exec <app> <args>'`. */
export interface RelaunchSpec {
  execPath: string;
  args: string[];
}

const SG = "/usr/bin/sg";

/** The real system. `relaunch` starts `spec` after this app exits and quits it (Electron's `app.relaunch` and `app.exit`). */
export function nodeLinuxSystem(relaunch: (spec: RelaunchSpec) => void, env: NodeJS.ProcessEnv = process.env): LinuxSystem {
  return {
    user: os.userInfo().username,
    groupEntry: () =>
      new Promise((resolve) => {
        execFile("getent", ["group", "docker"], { timeout: 5000 }, (error, stdout) => resolve(error ? null : String(stdout).trim() || null));
      }),
    status: () => readFile("/proc/self/status", "utf8"),
    socketExists: () => existsSync("/var/run/docker.sock") || existsSync("/run/docker.sock"),
    hasSg: () => existsSync(SG),
    relaunch: () => relaunch(sgRelaunch(env.APPIMAGE || process.execPath, process.argv.slice(1), SG)),
  };
}

/**
 * Whether the process `pid` lacks the `docker` group that this process has
 * (it started before the user joined the group). False when anything is
 * unknown: only a clear mismatch replaces a daemon.
 */
export async function daemonLacksDockerGroup(pid: number, read: (file: string) => Promise<string> = (f) => readFile(f, "utf8")): Promise<boolean> {
  try {
    const [entry, mine, theirs] = await Promise.all([
      new Promise<string | null>((resolve) => {
        execFile("getent", ["group", "docker"], { timeout: 5000 }, (error, stdout) => resolve(error ? null : String(stdout).trim() || null));
      }),
      read("/proc/self/status"),
      read(`/proc/${pid}/status`),
    ]);
    const { gid } = parseGroupEntry(entry, "");
    return gid !== null && statusGids(mine).includes(gid) && !statusGids(theirs).includes(gid);
  } catch {
    return false;
  }
}

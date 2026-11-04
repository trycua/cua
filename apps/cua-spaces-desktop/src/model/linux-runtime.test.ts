// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";
import {
  LinuxRuntime,
  SG_DOCKER_ENV,
  daemonLacksDockerGroup,
  linuxRuntimeAdvice,
  needsGroupRefresh,
  parseGroupEntry,
  sgRelaunch,
  statusGids,
  type LinuxSystem,
} from "./linux-runtime";

const STATUS_OLD = "Name:\tcua\nGid:\t1000\t1000\t1000\t1000\nGroups:\t4 24 27 1000 \n";
const STATUS_NEW = "Name:\tcua\nGid:\t1000\t1000\t1000\t1000\nGroups:\t4 24 27 118 1000 \n";

function system(over: { entry?: string | null; status?: string; socketExists?: () => boolean; hasSg?: () => boolean } = {}): LinuxSystem & { relaunch: ReturnType<typeof vi.fn> } {
  return {
    user: "azureuser",
    groupEntry: async () => (over.entry === undefined ? "docker:x:118:azureuser" : over.entry),
    status: async () => over.status ?? STATUS_OLD,
    socketExists: over.socketExists ?? (() => true),
    hasSg: over.hasSg ?? (() => true),
    relaunch: vi.fn(),
  } as LinuxSystem & { relaunch: ReturnType<typeof vi.fn> };
}

describe("group facts", () => {
  it("reads the docker group's id and members", () => {
    expect(parseGroupEntry("docker:x:118:alice,azureuser", "azureuser")).toEqual({ gid: 118, member: true });
    expect(parseGroupEntry("docker:x:118:alice", "azureuser")).toEqual({ gid: 118, member: false });
    expect(parseGroupEntry("docker:x:118:", "azureuser").member).toBe(false);
    expect(parseGroupEntry(null, "azureuser")).toEqual({ gid: null, member: false });
    expect(parseGroupEntry("garbage", "azureuser")).toEqual({ gid: null, member: false });
  });

  it("reads every group id a process has", () => {
    expect(statusGids(STATUS_OLD).sort((a, b) => a - b)).toEqual([4, 24, 27, 1000]);
    expect(statusGids(STATUS_NEW)).toContain(118);
    expect(statusGids("")).toEqual([]);
  });

  it("needs a refresh only for a member whose process predates the group", () => {
    expect(needsGroupRefresh({ gid: 118, member: true, sessionHas: false, socket: true })).toBe(true);
    expect(needsGroupRefresh({ gid: 118, member: true, sessionHas: true, socket: true })).toBe(false);
    expect(needsGroupRefresh({ gid: 118, member: false, sessionHas: false, socket: true })).toBe(false);
    expect(needsGroupRefresh({ gid: null, member: false, sessionHas: false, socket: false })).toBe(false);
  });
});

describe("linuxRuntimeAdvice", () => {
  const stale = { gid: 118, member: true, sessionHas: false, socket: true };
  it("offers a restart for a member whose session predates the group (CUA-1763)", () => {
    const a = linuxRuntimeAdvice(stale, "permission denied on unix:///var/run/docker.sock: add yourself", true, false);
    expect(a?.fix).toMatchObject({ action: "restart-app", label: "Restart Cua Spaces", busy: false });
    expect(a?.reason).toMatch(/no logout or reboot/);
  });

  it("offers the restart even when the doctor's words did not say permission denied", () => {
    expect(linuxRuntimeAdvice(stale, "container engine did not answer", true, false)?.fix?.action).toBe("restart-app");
  });

  it("says to log out when the restart already ran, or there is no sg", () => {
    for (const [can, again] of [[true, true], [false, false]] as const) {
      const a = linuxRuntimeAdvice(stale, "permission denied", can, again);
      expect(a?.fix).toBeNull();
      expect(a?.reason).toMatch(/Log out/);
    }
  });

  it("names the one command for a user who is not in the group", () => {
    const a = linuxRuntimeAdvice({ gid: 118, member: false, sessionHas: false, socket: true }, "permission denied on x", true, false);
    expect(a?.fix).toBeNull();
    expect(a?.reason).toContain("sudo usermod -aG docker $USER");
  });

  it("stays out of the way when Docker is simply not installed or not running", () => {
    expect(linuxRuntimeAdvice({ gid: null, member: false, sessionHas: false, socket: false }, "no Docker-API socket found", true, false)).toBeNull();
    expect(linuxRuntimeAdvice({ gid: 118, member: true, sessionHas: true, socket: true }, "container engine at x is not running", true, false)).toBeNull();
  });
});

describe("sgRelaunch", () => {
  it("runs the app under the docker group with the marker set, quoting every part", () => {
    const r = sgRelaunch("/opt/Cua Spaces/cua-spaces", ["--no-sandbox", "it's"], "/usr/bin/sg");
    expect(r.execPath).toBe("/usr/bin/sg");
    expect(r.args.slice(0, 2)).toEqual(["docker", "-c"]);
    expect(r.args[2]).toBe(`${SG_DOCKER_ENV}=1 exec '/opt/Cua Spaces/cua-spaces' '--no-sandbox' 'it'\\''s'`);
  });
});

describe("LinuxRuntime", () => {
  it("starts again under the group at launch for a stale session with Docker installed", async () => {
    const sys = system();
    expect(await new LinuxRuntime(sys, {}).relaunchIfNeeded()).toBe(true);
    expect(sys.relaunch).toHaveBeenCalledTimes(1);
  });

  it("does not relaunch when the session has the group, Docker is absent, the user is not a member, or it already did", async () => {
    for (const sys of [system({ status: STATUS_NEW }), system({ socketExists: () => false }), system({ entry: "docker:x:118:other" }), system({ entry: null }), system({ hasSg: () => false })]) {
      expect(await new LinuxRuntime(sys, {}).relaunchIfNeeded()).toBe(false);
      expect(sys.relaunch).not.toHaveBeenCalled();
    }
    const sys = system();
    expect(await new LinuxRuntime(sys, { [SG_DOCKER_ENV]: "1" }).relaunchIfNeeded()).toBe(false);
    expect(sys.relaunch).not.toHaveBeenCalled();
  });

  it("advises and runs the restart button once", async () => {
    const sys = system();
    const r = new LinuxRuntime(sys, {});
    expect((await r.advice("permission denied"))?.fix?.action).toBe("restart-app");
    r.run("restart-app");
    r.run("restart-app");
    expect(sys.relaunch).toHaveBeenCalledTimes(1);
  });
});

describe("daemonLacksDockerGroup", () => {
  it("is unknown-safe: false when a status file cannot be read", async () => {
    expect(await daemonLacksDockerGroup(999999, async () => Promise.reject(new Error("gone")))).toBe(false);
  });
});

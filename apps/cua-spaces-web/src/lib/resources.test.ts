// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { ComputerResources, SpaceResources } from "@/bridge";
import { DEMO_DISK_FREE, demoDiskWarning, demoResources } from "@/bridge/adapters/demo/resources";
import { demoSpaceRows } from "@/bridge/adapters/demo-data";
import { diskParts, extraSpacesLine, hostMeters, lowDiskWarning, onHostLine, size, sizeOf, spaceMeters, updatedAgo } from "./resources";

const GB = 1_000_000_000;

const host = (free: number, total = 1000 * GB): ComputerResources => ({
  id: "this-mac",
  name: "",
  online: true,
  sampledAtMs: 0,
  cpuCount: 12,
  cpuPercent: 38,
  loadAverage1m: 4,
  memoryTotal: 64 * GB,
  memoryUsed: 41 * GB,
  diskTotal: total,
  diskFree: free,
  spacesCount: 2,
  spacesRunning: 1,
  spacesCpuAllocated: 6,
  spacesMemoryAllocated: 12 * GB,
  spacesMemoryUsed: 3 * GB,
  spacesDiskBytes: 25 * GB,
  spacesDiskExact: false,
  warning: demoDiskWarning(free, total),
});

const space = (over: Partial<SpaceResources> = {}): SpaceResources => ({
  id: "local:macos-1",
  powerState: "running",
  cpuAllocated: 4,
  memoryAllocated: 8 * GB,
  diskAllocated: 80 * GB,
  usage: { cpuCount: 4, cpuPercent: 22, loadAverage1m: 1, memoryUsed: 3.1 * GB, memoryTotal: 8 * GB, memoryLimited: true, diskUsed: 34 * GB, diskTotal: 80 * GB, diskLimited: true },
  diskOnHost: { bytes: 19 * GB, allocated: 34 * GB, sharesSpace: true },
  ...over,
});

describe("sizes", () => {
  it("reads as people say them", () => {
    expect(size(41 * GB)).toBe("41 GB");
    expect(size(3.1 * GB)).toBe("3.1 GB");
    expect(size(8 * GB)).toBe("8 GB");
    expect(size(1000 * GB)).toBe("1 TB");
    expect(size(1500 * GB)).toBe("1.5 TB");
    expect(sizeOf(3.1 * GB, 8 * GB)).toBe("3.1 of 8 GB");
    expect(sizeOf(800 * GB, 1000 * GB)).toBe("800 GB of 1 TB");
  });
});

describe("the low-disk line", () => {
  it("the demo follows the daemon's rule: 50 GB or 15%, whichever is higher, capped at 100 GB", () => {
    const level = (free: number, total: number) => demoDiskWarning(free * GB, total * GB).level;
    expect(level(120, 1000)).toBe("ok");
    expect(level(90, 1000)).toBe("low");
    expect(level(45, 256)).toBe("low");
    expect(level(60, 256)).toBe("ok");
    expect(level(70, 500)).toBe("low");
    expect(level(4, 1000)).toBe("critical");
  });

  it("follows the host's level, and says it in plain words", () => {
    expect(lowDiskWarning({ ...host(300 * GB), warning: { level: "low", free: 300 * GB, total: 1000 * GB, threshold: 100 * GB } }, "This Mac")?.level).toBe("low");
    expect(lowDiskWarning({ ...host(41 * GB), warning: { level: "ok", free: 41 * GB, total: 1000 * GB, threshold: 0 } }, "This Mac")).toBeNull();
    expect(lowDiskWarning(host(41 * GB), "This Mac")).toEqual({
      level: "low",
      title: "This Mac is almost out of space. Delete old Spaces or snapshots.",
      detail: "41 GB free of 1 TB. New Spaces may not fit, and apps can slow down.",
    });
    expect(lowDiskWarning(host(3 * GB), "This Mac")?.title).toBe("This Mac is out of space. Delete old Spaces or snapshots.");
    expect(lowDiskWarning(host(300 * GB), "This Mac")).toBeNull();
  });
});

describe("meters", () => {
  it("This Mac: processor, memory and space for files", () => {
    const [cpu, mem, disk] = hostMeters(host(41 * GB));
    expect([cpu!.label, cpu!.value]).toEqual(["Processor", "12 cores · 38% busy"]);
    expect([mem!.label, mem!.value]).toEqual(["Memory", "41 of 64 GB in use"]);
    expect([disk!.label, disk!.value, disk!.tone]).toEqual(["Space for files", "41 GB free of 1 TB", "warning"]);
    expect(hostMeters(host(300 * GB))[2]!.tone).toBeUndefined();
    expect(extraSpacesLine(host(41 * GB), "here")).toBe("Your extra Spaces use 6 cores, 12 GB of memory and about 25 GB for files.");
    expect(extraSpacesLine({ ...host(41 * GB), spacesCount: 0 }, "here")).toBeNull();
  });

  it("a Space: in use now, and a stopped one keeps its disk", () => {
    expect(spaceMeters(space()).map((m) => m.value)).toEqual(["22% busy", "3.1 of 8 GB", "About 19 GB"]);
    expect(spaceMeters(space())[2]!.help).toBe("Uses about 19 GB on this Mac (shares space with other copies).");
    const stopped = spaceMeters(space({ powerState: "stopped", usage: null }));
    expect(stopped.map((m) => m.value)).toEqual(["Stopped", "Off", "About 19 GB"]);
    expect(stopped[0]!.fill).toBe(0);
  });

  it("is honest about shared disk", () => {
    expect(onHostLine(space())).toBe("Uses about 19 GB on this Mac (shares space with other copies).");
    expect(onHostLine(space({ diskOnHost: { bytes: 6 * GB, allocated: 6 * GB, sharesSpace: false } }))).toBe("Uses about 6 GB on this Mac.");
    // Couldn't measure, but shared; a container: nothing.
    expect(onHostLine(space({ diskOnHost: { bytes: null, allocated: 30 * GB, sharesSpace: true } }))).toBe("Shares space with other copies.");
    expect(onHostLine(space({ diskOnHost: null }))).toBeNull();
  });

  it("splits the disk into your files, each Space, and free", () => {
    const parts = diskParts(host(41 * GB), [{ name: "macOS 1", bytes: 19 * GB }, { name: "Linux 1", bytes: 6 * GB }]);
    expect(parts.own).toBe(934 * GB);
    expect(parts.free).toBe(41 * GB);
  });

  it("says how old another computer's summary is", () => {
    expect(updatedAgo(0, 30_000)).toBe("just now");
    expect(updatedAgo(0, 2 * 60_000)).toBe("2 min ago");
    expect(updatedAgo(0, 3 * 3_600_000)).toBe("3 hours ago");
  });
});

describe("the demo host's numbers", () => {
  it("cover This Mac's Spaces and sum what they use", () => {
    const snap = demoResources(demoSpaceRows(0), 0, DEMO_DISK_FREE.low);
    expect([snap.thisMac.id, ...snap.computers.map((c) => c.id)]).toEqual(["this-mac", "mac-mini"]);
    const here = snap.spaces.filter((s) => s.id.startsWith("local:"));
    expect(here.map((s) => s.id)).toEqual(["local:design-review", "local:agent-sandbox"]);
    expect(snap.thisMac.spacesDiskBytes).toBe(25 * GB);
    expect(snap.warning.level).toBe("low");
  });

  it("give every Space of yours on another online computer a size there, stopped ones too", () => {
    const snap = demoResources(demoSpaceRows(0), 0);
    const remote = snap.spaces.filter((s) => !s.id.startsWith("local:"));
    expect(remote.map((s) => [s.id, s.powerState, s.diskOnHost?.bytes])).toEqual([
      ["relay:mac-mini/release-checks", "suspended", 21 * GB],
      ["relay:mac-mini/qa-windows", "running", 25 * GB],
      ["relay:linux-box/ubuntu-build", "running", 5 * GB],
      ["relay:linux-box/windows-legacy", "suspended", 31 * GB],
    ]);
    // Only the disk is known there; this Mac's totals leave them out.
    expect(remote.every((s) => s.usage === null && s.cpuAllocated === 0)).toBe(true);
    expect(snap.thisMac.spacesCount).toBe(2);
  });

  it("give a Space on an offline computer no size", () => {
    const rows = [...demoSpaceRows(0), { ...demoSpaceRows(0)[2]!, id: "relay:studio-pc/render", host: "studio-pc" }];
    expect(demoResources(rows, 0).spaces.some((s) => s.id === "relay:studio-pc/render")).toBe(false);
  });
});

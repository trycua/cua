// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { ComputerResources, SpaceResources } from "@/bridge";
import { DEMO_DISK_FREE, demoDiskWarning, demoResources } from "@/bridge/adapters/demo/resources";
import { demoSpaceRows } from "@/bridge/adapters/demo-data";
import { diskFit, extraSpacesLine, hostMeters, knownSize, lowDiskWarning, onHostLine, size, sizeOf, spaceMeters, spacePanel, systemName, updatedAgo } from "./resources";

/** GiB: sizes read the way macOS shows memory ("4 GB of 36 GB"). */
const GB = 1024 ** 3;
const MB = 1024 ** 2;

const host = (free: number, total = 1024 * GB): ComputerResources => ({
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
  base: null,
  ...over,
});

describe("sizes", () => {
  it("reads as people say them", () => {
    expect(size(41 * GB)).toBe("41 GB");
    expect(size(3.1 * GB)).toBe("3.1 GB");
    expect(size(8 * GB)).toBe("8 GB");
    expect(size(1024 * GB)).toBe("1 TB");
    expect(size(1536 * GB)).toBe("1.5 TB");
    expect(sizeOf(3.1 * GB, 8 * GB)).toBe("3.1 of 8 GB");
    expect(sizeOf(800 * GB, 1024 * GB)).toBe("800 GB of 1 TB");
  });

  it("uses one convention, the way macOS shows memory: 4 GiB is 4 GB, never 4.3", () => {
    expect(size(4 * GB)).toBe("4 GB");
    expect(size(36 * GB)).toBe("36 GB");
    // A 2 TB (decimal) disk, in the same binary units.
    expect(size(2e12)).toBe("1.8 TB");
  });

  it("never says 0 GB for something", () => {
    expect(size(500 * MB)).toBe("500 MB");
    expect(size(249_856)).toBe("244 KB");
    expect(size(0)).toBe("0 KB");
    expect(sizeOf(717 * MB, 4 * GB)).toBe("717 MB of 4 GB");
  });

  it("a sentinel is no size", () => {
    for (const v of [2 ** 63 - 1, 9223372036854771712, 2 ** 64 - 1, -1, 0, Number.NaN, null, undefined]) expect(knownSize(v)).toBeNull();
    expect(knownSize(80 * GB)).toBe(80 * GB);
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

  it("names a base by its system", () => {
    expect(systemName("Ubuntu 24.04.5 LTS")).toBe("Ubuntu 24.04");
    expect(systemName("macOS 26.5.2 (25F84)")).toBe("macOS 26.5");
    expect(systemName(undefined)).toBeNull();
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

/** The owner's Mac: 18 cores, 36 GiB of memory, a 2 TB disk nearly full. */
const ownersMac = (): ComputerResources => ({
  ...host(5.6e9, 2e12),
  cpuCount: 18,
  memoryTotal: 36 * GB,
  warning: { level: "critical", free: 5.6e9, total: 2e12, threshold: 5 * GB },
});

/** pipbox: an Ubuntu container under gVisor, whose root has no size of its own. */
const pipbox = (diskTotal: number | null): SpaceResources => ({
  id: "local:pipbox",
  powerState: "running",
  cpuAllocated: 2,
  memoryAllocated: 4 * GB,
  diskAllocated: 0,
  usage: { cpuCount: 2, cpuPercent: 13, loadAverage1m: 0, memoryUsed: 717 * MB, memoryTotal: 4 * GB, memoryLimited: true, diskUsed: 249_856, diskTotal, diskLimited: false },
  diskOnHost: { bytes: 249_856, allocated: 249_856, sharesSpace: false },
  base: { key: "sha256:9ac0", image: "ghcr.io/trycua/linux:24.04", bytes: 4.41e9, sharedWith: 1 },
});

describe("a Space's resources dialog (the core's stand-in)", () => {
  const view = (s: SpaceResources, spaces = [s]) => spacePanel({ space: s, name: "pipbox", host: ownersMac(), computer: "this Mac", spaces, baseName: "Ubuntu 24.04" });

  it("no sentinel ever reaches it: i64::MAX, u64::MAX, -1, 0", () => {
    for (const sentinel of [2 ** 63 - 1, 9223372036854771712, 2 ** 64 - 1, -1, 0, null]) {
      const v = view(pipbox(sentinel));
      expect(v.cards.map((c) => [c.label, c.value, c.given])).toEqual([
        ["Processor", "13% busy", "Given 2 of this Mac’s 18 cores"],
        ["Memory", "717 MB of 4 GB", "Given 4 GB of this Mac’s 36 GB"],
        ["Space for files", "244 KB", null],
      ]);
      expect(v.cards[2]!.fraction).toBeNull();
      expect(JSON.stringify(v)).not.toMatch(/9223372|PB|Busy means|Uses about|Updates every/);
    }
  });

  it("how it fits adds up, the base counted once", () => {
    const fit = view(pipbox(null)).fit!;
    expect(fit.total).toBe("1.8 TB");
    expect(fit.parts.map((p) => `${p.label} · ${p.value}${p.detail ? `, ${p.detail}` : ""}`)).toEqual([
      "Your own files and apps · 1.8 TB",
      "pipbox · 244 KB",
      "Ubuntu 24.04 image · 4.1 GB, only this Space uses it",
      "Free · 5.2 GB",
    ]);
    expect(fit.parts.reduce((n, p) => n + p.bytes, 0)).toBe(2e12);
    expect(fit.parts.reduce((n, p) => n + p.fraction, 0)).toBeCloseTo(1, 9);
    expect(fit.warning?.critical).toBe(true);
  });

  it("copy-on-write: two Spaces on one image count it once and say so", () => {
    const base = { key: "sha256:9ac0", image: "ghcr.io/trycua/linux:24.04", bytes: 4.41e9, sharedWith: 2 };
    const me = { ...pipbox(null), base };
    const twin: SpaceResources = { ...me, id: "local:twin", diskOnHost: { bytes: 2 * GB, allocated: 2 * GB, sharesSpace: false } };
    const fit = diskFit(ownersMac(), me, "pipbox", [me, twin], "Ubuntu 24.04");
    expect(fit.parts.filter((p) => p.kind === "base").map((p) => p.detail)).toEqual(["shared by 2 Spaces"]);
    expect(fit.parts.find((p) => p.kind === "otherSpaces")?.value).toBe("2 GB");
    expect(fit.parts.reduce((n, p) => n + p.bytes, 0)).toBe(2e12);
  });

  it("not copy-on-write: only its own data", () => {
    const me = { ...space(), base: null };
    const fit = diskFit(host(41 * GB), me, "macOS 1", [me], null);
    expect(fit.parts.some((p) => p.kind === "base")).toBe(false);
    expect(fit.parts.find((p) => p.kind === "thisSpace")?.value).toBe("19 GB");
  });

  it("what can't be told apart is said plainly, never guessed", () => {
    const me = space({ diskOnHost: { bytes: null, allocated: 40 * GB, sharesSpace: true }, base: { key: "m", image: "ghcr.io/trycua/macos:26", bytes: null, sharedWith: 1 } });
    const fit = diskFit(host(41 * GB), me, "macOS 1", [me], null);
    expect(fit.note).toBe("Can’t tell macOS 1’s own data apart from the image it shares.");
    expect(fit.parts.some((p) => p.kind === "thisSpace" || p.kind === "base")).toBe(false);
    const files = spacePanel({ space: me, name: "macOS 1", host: null, computer: "this Mac", spaces: [me], baseName: null }).cards[2]!;
    expect(files.value).toBe("Up to 80 GB");
  });
});

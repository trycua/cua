// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { Machine, Space } from "@/bridge";
import { busyMacosSpaces, canDelete, canPower, createError, isMachineDesktop, machineName, macosLimitText, osLabel, powerPending, realSpaces, spacePlace, spaceState, visibleSpaces } from "./spaces";

const space = (id: string, status: Space["status"], lastUsedAt: number, extra: Partial<Space> = {}): Space => ({
  id,
  name: id,
  os: "linux",
  status,
  detail: "",
  lastUsedAt,
  scene: "linux-terminal",
  provider: "relay",
  ...extra,
});

const SPACES: Space[] = [
  space("off", "suspended", 5),
  space("on-old", "running", 1),
  space("new", "provisioning", 2),
  space("on-new", "running", 9),
  space("powered-off", "running", 7, { power: { control: "stop", off: true } }),
];

const MACHINES: Machine[] = [
  { id: "mac-mini", name: "Mac mini", os: "macos", via: "relay", online: true, current: false, spaceIds: ["on-old"], limits: [] },
];

describe("spaceState", () => {
  it("maps the bridge's statuses onto the screen's", () => {
    expect(SPACES.map(spaceState)).toEqual(["stopped", "running", "creating", "running", "stopped"]);
  });
});

describe("a failed create", () => {
  const progress = (error?: string) => ({ phase: "booting", permille: 900, label: "Failed", error, cancellable: false, cancelling: false });
  const failed = space("pending:m", "suspended", 3, { os: "macos", provider: "local", progress: progress("Apple allows two macOS VMs at once.") });

  it("reads Failed with its reason, first in the list, and is removed rather than deleted", () => {
    expect(spaceState(failed)).toBe("failed");
    expect(createError(failed)).toBe("Apple allows two macOS VMs at once.");
    expect(canDelete(failed)).toBe(false);
    expect(visibleSpaces([...SPACES, failed], "all")[0]!.id).toBe("pending:m");
    expect(visibleSpaces([...SPACES, failed], "stopped").map((s) => s.id)).not.toContain("pending:m");
    // A stopped Space has no create error.
    expect(createError(SPACES[0]!)).toBeNull();
    expect(spaceState(space("pending:x", "provisioning", 1, { progress: progress() }))).toBe("creating");
  });
});

describe("macosLimitText", () => {
  const mac = (id: string, status: Space["status"], extra: Partial<Space> = {}) => space(id, status, 1, { os: "macos", provider: "local", ...extra });
  const core = (answer: string | null) => {
    const asked: unknown[] = [];
    return { asked, client: { status: "ready", tryCall: (m: string, a: unknown) => (asked.push([m, a]), answer) } as never };
  };

  it("counts this Mac's running and creating macOS Spaces, not stopped, other machines' or Linux ones", () => {
    expect(busyMacosSpaces([mac("a", "running"), mac("b", "provisioning")])).toBe(2);
    expect(busyMacosSpaces([mac("a", "running"), mac("b", "suspended"), mac("c", "running", { provider: "relay" }), mac("d", "running", { os: "linux" })])).toBe(1);
    expect(busyMacosSpaces([mac("a", "running"), mac("b", "running", { power: { control: "stop", off: true } })])).toBe(1);
  });

  it("asks the core with the Spaces and the host's macOS VMs, and says nothing without it", () => {
    const c = core("This Mac is already running 2 macOS virtual machines, …");
    expect(macosLimitText([mac("a", "running")], 2, c.client)).toBe("This Mac is already running 2 macOS virtual machines, …");
    expect(c.asked).toEqual([["wizard.macosLimit", { spacesBusy: 1, vmsRunning: 2 }]]);
    expect(macosLimitText([], undefined, core(null).client)).toBeNull();
    expect(macosLimitText([], 2, { status: "unavailable" } as never)).toBeNull();
  });
});

describe("visibleSpaces", () => {
  it("orders creating, then running, then stopped, most recent first", () => {
    expect(visibleSpaces(SPACES, "all").map((s) => s.id)).toEqual(["new", "on-new", "on-old", "powered-off", "off"]);
  });

  it("filters by state", () => {
    expect(visibleSpaces(SPACES, "stopped").map((s) => s.id)).toEqual(["powered-off", "off"]);
  });
});

describe("machineName", () => {
  it("names the machine a Space runs on", () => {
    expect(machineName(MACHINES, SPACES[1]!)).toBe("Mac mini");
    expect(machineName(MACHINES, { id: "x", provider: "cloud" })).toBe("Cua Cloud");
    expect(machineName(MACHINES, { id: "x", provider: "relay", hostName: "Linux box" })).toBe("Linux box");
    expect(machineName(MACHINES, { id: "x", provider: "relay" })).toBe("Unknown machine");
  });
});

describe("space actions", () => {
  const powered = space("p", "running", 1, { power: { control: "stop", off: false } });

  it("powers only Spaces that report power", () => {
    expect(canPower(powered)).toBe(true);
    expect(canPower(space("c", "running", 1))).toBe(false);
    expect(canPower({ ...powered, status: "deleting" })).toBe(false);
  });

  it("names the power action in flight", () => {
    expect(powerPending(powered)).toBeNull();
    expect(powerPending({ power: { control: "suspend", off: false, turningOn: false } })).toBe("Suspending…");
    expect(powerPending({ power: { control: "stop", off: true, turningOn: true } })).toBe("Starting…");
  });

  it("never deletes this Mac or a Space deletable only elsewhere", () => {
    expect(canDelete(powered)).toBe(true);
    expect(canDelete(space("this-mac", "local", 1))).toBe(false);
    expect(canDelete({ ...powered, cloudDelete: "elsewhere" })).toBe(false);
  });
});

describe("isMachineDesktop / realSpaces", () => {
  it("leaves this Mac and relay machines' own desktops out of the Spaces", () => {
    const rows: Space[] = [
      space("this-mac", "suspended", 1, { provider: "local", name: "This machine" }),
      space("local-desktop", "local", 1, { provider: "local" }),
      space("relay:96fedb7e1be65c3d31fa18587febde2c", "running", 1, { name: "gamma-4 Mac Studio" }),
      space("relay:space-ca12d6d23907f387", "running", 1, { host: "96fedb7e1be65c3d31fa18587febde2c" }),
      space("relay:mac-mini/release-checks", "running", 1, { host: "mac-mini" }),
      space("local:qa-gui-linux-local", "running", 1, { provider: "local" }),
      space("pending:1", "provisioning", 1, { provider: "local" }),
    ];
    expect(rows.filter(isMachineDesktop).map((s) => s.id)).toEqual(["this-mac", "local-desktop", "relay:96fedb7e1be65c3d31fa18587febde2c"]);
    expect(realSpaces(rows).map((s) => s.id)).toEqual(["relay:space-ca12d6d23907f387", "relay:mac-mini/release-checks", "local:qa-gui-linux-local", "pending:1"]);
  });
});

describe("an unknown OS", () => {
  it("is never named Linux, and the place reads as the machine alone", () => {
    const unknown = space("relay:x", "running", 1, { os: "unknown", scene: "blank" });
    expect(osLabel(unknown)).toBe("");
    expect(spacePlace(unknown, "Studio")).toBe("Studio");
    expect(spacePlace(space("a", "running", 1), "Studio")).toBe("Linux on Studio");
    expect(spacePlace(space("a", "running", 1, { os: "macos", osPrettyName: "macOS Tahoe 26.0" }), "Studio")).toBe("macOS Tahoe 26.0 on Studio");
  });
});

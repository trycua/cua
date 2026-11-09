// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { Machine, Space } from "@/bridge";
import { busyMacosSpaces, canDelete, canPower, countedSpaces, createError, isUnsharedDesktop, listedSpaces, machineName, macosLimitText, orderSpaces, osLabel, powerButton, powerPending, realSpaces, spacePlace, spaceState, visibleSpaces, isMachineDesktop } from "./spaces";

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

  it("reads Failed with its reason, after the running Spaces, and is removed rather than deleted", () => {
    expect(spaceState(failed)).toBe("failed");
    expect(createError(failed)).toBe("Apple allows two macOS VMs at once.");
    expect(canDelete(failed)).toBe(false);
    expect(visibleSpaces([...SPACES, failed], "all").map((s) => s.id)).toEqual(["on-new", "on-old", "new", "pending:m", "powered-off", "off"]);
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
  it("orders running, then creating, then stopped, most recent first", () => {
    expect(visibleSpaces(SPACES, "all").map((s) => s.id)).toEqual(["on-new", "on-old", "new", "powered-off", "off"]);
  });

  it("filters by state", () => {
    expect(visibleSpaces(SPACES, "stopped").map((s) => s.id)).toEqual(["powered-off", "off"]);
  });
});

describe("machineName", () => {
  it("names the machine a Space runs on", () => {
    expect(machineName(MACHINES, SPACES[1]!)).toBe("Mac mini");
    expect(machineName(MACHINES, { id: "x", provider: "direct" })).toBe("a computer added by address");
    expect(machineName(MACHINES, { id: "x", provider: "relay", hostName: "Linux box" })).toBe("Linux box");
    expect(machineName(MACHINES, { id: "x", provider: "relay" })).toBe("Unknown computer");
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
      space("this-mac", "suspended", 1, { provider: "local", name: "This computer" }),
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

describe("powerButton", () => {
  const noCore = { tryCall: () => undefined };
  const on = space("p", "running", 1, { power: { control: "suspend", off: false } });

  it("follows the core when it answers", () => {
    const core = { tryCall: <T,>() => ({ symbol: "power", help: { code: "spaces.power.suspend", vars: {} }, turnOn: false, enabled: true, busy: false }) as T };
    expect(powerButton(core, on)).toEqual({ help: "Suspend", turnOn: false, busy: false });
    expect(powerButton({ tryCall: <T,>() => null as T }, on)).toBeNull();
  });

  it("without the core, has the core's rule and words", () => {
    expect(powerButton(noCore, on)).toEqual({ help: "Suspend", turnOn: false, busy: false });
    expect(powerButton(noCore, { ...on, power: { control: "stop", off: true } })).toEqual({ help: "Turn on", turnOn: true, busy: false });
    expect(powerButton(noCore, { ...on, power: { control: "stop", off: false, turningOn: false } })).toEqual({ help: "Turning off…", turnOn: false, busy: true });
    expect(powerButton(noCore, space("c", "running", 1))).toBeNull();
    expect(powerButton(noCore, { ...on, status: "deleting" })).toBeNull();
    expect(powerButton(noCore, { ...on, id: "this-mac" })).toBeNull();
    expect(powerButton(noCore, { ...on, progress: { phase: "pulling", permille: 100, label: "Downloading" } })).toBeNull();
  });
});

describe("the Spaces list's desktop rows", () => {
  const real = (id: string, lastUsedAt = 1) => space(id, "running", lastUsedAt, { provider: "local", id });
  /** This Mac's desktop: running (reachable) while it is shared, suspended while it is not. */
  const thisMac = (shared: boolean) => space("this-mac", shared ? "running" : "suspended", 1, { provider: "local" });
  const theirs = (machine: string) => space(`relay:${machine}`, "running", 1, { provider: "relay" });
  const ids = (list: readonly Space[]) => list.map((s) => s.id);

  it("tells This Mac's shared desktop from its unshared one", () => {
    expect(isUnsharedDesktop(thisMac(false))).toBe(true);
    expect(isUnsharedDesktop(thisMac(true))).toBe(false);
    expect(isUnsharedDesktop(real("a"))).toBe(false);
    expect(isMachineDesktop(thisMac(false))).toBe(true);
  });

  it("zero Spaces and not sharing: nothing listed, so the empty state shows", () => {
    expect(listedSpaces([thisMac(false)])).toEqual([]);
  });

  it("zero Spaces and sharing: the list holds the desktop row, and it counts", () => {
    const listed = listedSpaces([thisMac(true)]);
    expect(ids(listed)).toEqual(["this-mac"]);
    expect(countedSpaces(listed)).toHaveLength(1);
  });

  it("one Space and not sharing: both rows, but the unshared desktop is never counted", () => {
    const listed = listedSpaces([thisMac(false), real("a")]);
    expect(ids(listed)).toEqual(["a", "this-mac"]);
    expect(ids(countedSpaces(listed))).toEqual(["a"]);
  });

  it("Hide leaves a desktop out, and showing it again brings it back", () => {
    const all = [thisMac(true), real("a")];
    expect(ids(listedSpaces(all, new Set(["this-mac"])))).toEqual(["a"]);
    expect(ids(listedSpaces(all, new Set()))).toEqual(["a", "this-mac"]);
    // Hidden, an unshared desktop stays out too.
    expect(ids(listedSpaces([thisMac(false), real("a")], new Set(["this-mac"])))).toEqual(["a"]);
  });

  it("another machine's desktop shows only while it shares it", () => {
    const all = [real("a"), theirs("studio"), theirs("spare")];
    const shares = (s: Space) => s.id !== "relay:spare";
    expect(ids(listedSpaces(all, new Set(), shares))).toEqual(["a", "relay:studio"]);
  });
});

describe("orderSpaces: running first, then the most recently used", () => {
  it("puts a running Space before newer stopped ones", () => {
    const all = [
      space("stopped-new", "suspended", 90),
      space("stopped-newer", "suspended", 95),
      space("running-old", "running", 10),
      space("running-newest", "running", 80),
      space("being-made", "provisioning", 99),
    ];
    expect(orderSpaces(all).map((s) => s.id)).toEqual(["running-newest", "running-old", "being-made", "stopped-newer", "stopped-new"]);
  });

  it("lists a desktop after the Spaces in its state, and This Mac's unshared desktop last", () => {
    const all = [space("this-mac", "suspended", 999, { provider: "local" }), space("relay:studio", "running", 999, { provider: "relay" }), space("a", "running", 1), space("b", "suspended", 2)];
    expect(orderSpaces(all).map((s) => s.id)).toEqual(["a", "relay:studio", "b", "this-mac"]);
  });
});

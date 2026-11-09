// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { resolveDefaultSpace, type Machine, type Space } from "@/bridge";

import { buildSpacesTree, findNode, foldOffline, treeRows, treeSummary } from "./spaces-tree";

const NOW = Date.parse("2026-11-04T12:00:00Z");

const machine = (over: Partial<Machine>): Machine => ({
  id: "m",
  name: "M",
  os: "macos",
  via: "relay",
  connection: "relay",
  online: true,
  current: false,
  spaceIds: [],
  limits: [],
  ...over,
});
const space = (over: Partial<Space>): Space => ({
  id: "s",
  name: "s",
  os: "linux",
  status: "running",
  detail: "",
  lastUsedAt: 0,
  scene: "blank",
  provider: "relay",
  ...over,
});

const THIS_MAC = machine({ id: "this-mac", name: "Francesco's MacBook", current: true, via: "local", relayId: "mac-here" });
const STUDIO = machine({ id: "studio", name: "Studio Mac mini", lastSeen: Math.floor((NOW - 20 * 60_000) / 1000) });
const DILLON = machine({ id: "dillon", name: "Dillon's PC", os: "windows" });

/** f4: 3 computers and 4 Spaces made on them. */
const SPACES: Space[] = [
  space({ id: "this-mac", name: "This Mac", status: "local", provider: "local", os: "macos" }),
  space({ id: "local:linux-test-box", name: "Linux test box", provider: "local" }),
  space({ id: "relay:studio", name: "Studio Mac mini", os: "macos" }),
  space({ id: "relay:studio/safari-check", name: "Safari check", os: "macos", host: "studio", hostName: "Studio Mac mini" }),
  space({ id: "relay:studio/build-box", name: "Build box", host: "studio", hostName: "Studio Mac mini" }),
  space({ id: "relay:studio/release-runner", name: "Release runner", host: "studio", hostName: "Studio Mac mini", power: { control: "stop", off: true } }),
  space({ id: "relay:dillon", name: "Dillon's PC", os: "windows" }),
];
const RUNNING = new Map([
  ["local:linux-test-box", 1],
  ["relay:studio/safari-check", 1],
  ["relay:studio/build-box", 1],
  ["relay:dillon", 1],
]);

const tree = (over: Partial<Parameters<typeof buildSpacesTree>[0]> = {}) =>
  buildSpacesTree({ spaces: SPACES, machines: [THIS_MAC, STUDIO, DILLON], defaultSpace: null, running: RUNNING, me: "f@trycua.com", now: NOW, ...over });

describe("buildSpacesTree", () => {
  it("puts each computer first, This Mac at the top, and its Spaces under it", () => {
    const t = tree();
    expect(t.roots.map((r) => r.name)).toEqual(["This Mac", "Studio Mac mini", "Dillon's PC"]);
    expect(t.roots[0]!.children.map((c) => c.name)).toEqual(["Linux test box"]);
    expect(t.roots[1]!.children.map((c) => c.name)).toEqual(["Safari check", "Build box", "Release runner"]);
    expect(t.roots[2]!.children).toEqual([]);
    expect(t.roots[0]!.settingsId).toBe("relay:mac-here");
    expect(t.roots[1]!.children[0]!.parentKey).toBe("relay:studio");
  });

  it("counts computers, Spaces, online rows and running tasks (a computer adds up its Spaces')", () => {
    const t = tree();
    expect([t.computers, t.spaces, t.online]).toEqual([3, 4, 6]);
    expect(treeSummary(t)).toBe("3 computers and 4 Spaces made on them. 7 in all, 6 online.");
    expect(t.roots.map((r) => r.tasks)).toEqual(["1 running", "2 running", "1 running"]);
    expect(t.roots[1]!.children.map((c) => [c.status, c.tasks])).toEqual([
      ["Online", "1 running"],
      ["Online", "1 running"],
      ["Stopped", "None"],
    ]);
  });

  it("while none is set, an offline computer is never marked Default: the first online one is, else This Mac", () => {
    expect(tree({ machines: [THIS_MAC, { ...STUDIO, online: false }, DILLON] }).defaultKey).toBe("relay:dillon");
    const allOff = tree({ machines: [THIS_MAC, { ...STUDIO, online: false }, { ...DILLON, online: false }] });
    expect(allOff.defaultKey).toBe("relay:mac-here");
    expect(
      treeRows(allOff)
        .filter((n) => n.isDefault)
        .map((n) => n.name),
    ).toEqual(["This Mac"]);
  });

  it("marks the first online computer that isn't this Mac Default while none is set, and the setting once it is", () => {
    expect(tree().defaultKey).toBe("relay:studio");
    expect(tree().roots[1]!.status).toBe("Online, Default");
    const set = tree({ defaultSpace: "relay:dillon" });
    expect(set.defaultKey).toBe("relay:dillon");
    expect(
      treeRows(set)
        .filter((n) => n.isDefault)
        .map((n) => n.name),
    ).toEqual(["Dillon's PC"]);
    // A default that's no longer listed: the first other computer again.
    expect(tree({ defaultSpace: "relay:gone" }).defaultKey).toBe("relay:studio");
  });

  it("a computer the relay calls offline is offline, even while the machines list still says connected", () => {
    const t = tree({ defaultSpace: "relay:studio", relayOnline: new Map([["relay:studio", false]]) });
    expect(t.roots[1]!.online).toBe(false);
    expect(t.roots[1]!.status).toBe("Offline, seen 20 minutes ago, Default");
    expect(t.roots[1]!.children.every((c) => c.status === "Waits for Studio Mac mini")).toBe(true);
    // Unset: the relay's offline computer isn't the default either.
    expect(tree({ relayOnline: new Map([["relay:studio", false]]) }).defaultKey).toBe("relay:dillon");
    // The relay only ever turns a computer offline here, never online.
    expect(tree({ machines: [THIS_MAC, { ...STUDIO, online: false }, DILLON], relayOnline: new Map([["relay:studio", true]]) }).roots[1]!.online).toBe(false);
  });

  it("greys out the Spaces of an offline computer: they wait for it and their tasks are paused", () => {
    // Set as the default, it stays marked Default while offline (the fallback line says where tasks go instead).
    const t = tree({ machines: [THIS_MAC, { ...STUDIO, online: false }, DILLON], defaultSpace: "relay:studio" });
    const studio = t.roots[1]!;
    expect(studio.status).toBe("Offline, seen 20 minutes ago, Default");
    expect(studio.tasks).toBe("Paused");
    expect(studio.children.map((c) => [c.status, c.tasks, c.dim, c.online])).toEqual([
      ["Waits for Studio Mac mini", "Paused", true, false],
      ["Waits for Studio Mac mini", "Paused", true, false],
      ["Waits for Studio Mac mini", "None", true, false],
    ]);
    expect(t.offlineParent?.key).toBe("relay:studio");
    expect(treeSummary(t)).toBe("Studio Mac mini is offline, so the 3 Spaces on it are too. They come back when it does.");
  });

  it("lists a Space whose computer isn't one of yours as a top row of its own", () => {
    const shared = space({
      id: "relay:space-sam-qa",
      name: "Sam's QA",
      hostName: "Sam's Mac",
      access: {
        kind: "withMe",
        role: "editor",
        ownerName: "Sam",
        place: "Sam's Mac",
        people: [],
        canPower: false,
        canTeleport: false,
        canShare: false,
        canRename: false,
        canDelete: false,
        canLeave: true,
      },
    });
    const t = tree({ spaces: [...SPACES, shared] });
    expect(t.roots.map((r) => r.name)).toEqual(["This Mac", "Studio Mac mini", "Dillon's PC", "Sam's QA"]);
    const orphan = t.roots[3]!;
    expect([orphan.kind, orphan.parentKey, orphan.people]).toEqual(["space", null, ["Sam"]]);
    expect(t.spaces).toBe(5);
  });

  it("is empty before this Mac is added, and has one computer (the default) when it's the only one", () => {
    const unshared = { ...THIS_MAC, relayId: undefined, connection: null };
    const empty = buildSpacesTree({ spaces: [], machines: [unshared], defaultSpace: null, now: NOW });
    expect(empty.roots).toEqual([]);
    expect([empty.computers, empty.spaces]).toEqual([0, 0]);

    const single = buildSpacesTree({ spaces: [SPACES[0]!], machines: [THIS_MAC], defaultSpace: null, me: "f@trycua.com", now: NOW });
    expect(single.roots.map((r) => [r.name, r.status, r.tasks, r.people])).toEqual([["This Mac", "Online, Default", "None", ["f@trycua.com"]]]);
    expect(treeSummary(single)).toBe("1 computer. New tasks run on This Mac until you add another.");
  });

  it("leaves devices that are only signed in out of the tree", () => {
    const t = tree({ machines: [THIS_MAC, STUDIO, DILLON, machine({ id: "phone", name: "iPhone", device: true })] });
    expect(t.roots.map((r) => r.name)).not.toContain("iPhone");
  });
});

describe("foldOffline", () => {
  const old = (i: number) => machine({ id: `old-${i}`, name: `Old box ${i}`, online: false, os: "linux", lastSeen: Math.floor((NOW - 86_400_000) / 1000) });
  const oldSpaces = (n: number) => Array.from({ length: n }, (_, i) => space({ id: `relay:old-${i}`, name: `Old box ${i}` }));

  it("folds offline computers with nothing on them into one row at the end, keeping This Mac, the default and computers with Spaces", () => {
    const olds = [0, 1, 2, 3].map(old);
    const t = tree({ machines: [THIS_MAC, { ...STUDIO, online: false }, DILLON, ...olds], spaces: [...SPACES, ...oldSpaces(4)], defaultSpace: "relay:old-0" });
    const { shown, folded } = foldOffline(t.roots);
    // Studio is offline but has Spaces on it; Old box 0 is the default: both stay in the list.
    expect(shown.map((r) => r.name)).toEqual(["This Mac", "Studio Mac mini", "Dillon's PC", "Old box 0"]);
    expect(folded.map((r) => r.name)).toEqual(["Old box 1", "Old box 2", "Old box 3"]);
  });

  it("leaves a single offline computer where it is", () => {
    const t = tree({ machines: [THIS_MAC, STUDIO, DILLON, old(0)], spaces: [...SPACES, ...oldSpaces(1)] });
    const { shown, folded } = foldOffline(t.roots);
    expect(folded).toEqual([]);
    expect(shown.map((r) => r.name)).toEqual(["This Mac", "Studio Mac mini", "Dillon's PC", "Old box 0"]);
  });
});

describe("the default Space", () => {
  it("the resolver treats a saved default that's no longer a computer of yours as unset", () => {
    const candidates = [
      { space: "relay:studio", name: "Studio Mac mini", online: false },
      { space: "relay:mac-here", name: "This Mac", online: true, here: true },
    ];
    expect(resolveDefaultSpace({ defaultSpace: "relay:removed", fallback: [], fallbackOn: true }, candidates)).toMatchObject({
      space: "relay:mac-here",
      defaultSpace: null,
      defaultOnline: null,
    });
    // One that is still there is kept.
    expect(resolveDefaultSpace({ defaultSpace: "relay:studio", fallback: [], fallbackOn: true }, candidates)).toMatchObject({
      defaultSpace: "relay:studio",
      defaultOnline: false,
    });
  });
});

describe("findNode", () => {
  const t = tree({ defaultSpace: "relay:studio" });

  it("finds a row by its key or the relay's id", () => {
    expect(findNode(t, "relay:studio/build-box")?.name).toBe("Build box");
    expect(findNode(t, "relay:mac-here")?.name).toBe("This Mac");
  });
});

describe("a removed computer", () => {
  const ghost = space({ id: "relay:a77e7c1777f98e7", name: "a77e7c1777f98e7", status: "running" });
  const ghostChild = space({ id: "relay:a77e7c1777f98e7/box", name: "Build box", host: "a77e7c1777f98e7" });

  it("leaves no computer named by its raw id, nor its Spaces, when its machine is gone", () => {
    const t = tree({ spaces: [...SPACES, ghost, ghostChild] });
    expect(t.roots.map((r) => r.name)).toEqual(["This Mac", "Studio Mac mini", "Dillon's PC"]);
    expect([t.computers, t.spaces]).toEqual([3, 4]);
  });

  it("is not the default even when the setting still names it", () => {
    const t = tree({ spaces: [...SPACES, ghost, ghostChild], defaultSpace: ghost.id });
    expect(t.defaultKey).not.toBe(ghost.id);
    expect(t.roots.some((r) => r.isDefault && r.key === ghost.id)).toBe(false);
  });

  it("does not hide a teammate's Space whose own name is shown", () => {
    const t = tree({ spaces: [...SPACES, space({ id: "relay:space-sam-qa", name: "Sam's QA" })] });
    expect(t.roots.map((r) => r.name)).toContain("Sam's QA");
  });
});

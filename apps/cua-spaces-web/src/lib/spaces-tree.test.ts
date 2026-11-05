// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { Machine, Space } from "@/bridge";

import { currentTreeWhere, newSpaceTargets, treeDefault } from "./default-where";
import { buildSpacesTree, defaultCandidates, findNode, treeRows, treeSummary } from "./spaces-tree";

const NOW = Date.parse("2026-11-04T12:00:00Z");

const machine = (over: Partial<Machine>): Machine => ({ id: "m", name: "M", os: "macos", via: "relay", connection: "relay", online: true, current: false, spaceIds: [], limits: [], ...over });
const space = (over: Partial<Space>): Space => ({ id: "s", name: "s", os: "linux", status: "running", detail: "", lastUsedAt: 0, scene: "blank", provider: "relay", ...over });

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
    expect(treeRows(allOff).filter((n) => n.isDefault).map((n) => n.name)).toEqual(["This Mac"]);
  });

  it("marks the first online computer that isn't this Mac Default while none is set, and the setting once it is", () => {
    expect(tree().defaultKey).toBe("relay:studio");
    expect(tree().roots[1]!.status).toBe("Online, Default");
    const set = tree({ defaultSpace: "relay:dillon" });
    expect(set.defaultKey).toBe("relay:dillon");
    expect(treeRows(set).filter((n) => n.isDefault).map((n) => n.name)).toEqual(["Dillon's PC"]);
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
    const shared = space({ id: "relay:space-sam-qa", name: "Sam's QA", hostName: "Sam's Mac", access: { kind: "withMe", role: "editor", ownerName: "Sam", place: "Sam's Mac", people: [], canPower: false, canTeleport: false, canShare: false, canRename: false, canDelete: false, canLeave: true } });
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

describe("treeDefault", () => {
  const settings = (defaultSpace: string | null, fallback: string[] = [], fallbackOn = true) => ({ defaultSpace, fallback, fallbackOn, updatedAt: 0 });

  it("runs on the default while it's online, with no line under the box", () => {
    const t = tree({ defaultSpace: "relay:studio" });
    const d = treeDefault(t, settings("relay:studio"), null);
    expect(d.where).toEqual({ kind: "space", space: SPACES[2] });
    expect(d.fallback).toBeNull();
  });

  it("goes to the next online computer when the default is offline, and says why", () => {
    const t = tree({ defaultSpace: "relay:studio", machines: [THIS_MAC, { ...STUDIO, online: false }, DILLON] });
    const d = treeDefault(t, settings("relay:studio"), null);
    expect(d.where).toEqual({ kind: "space", space: SPACES[6] });
    expect(d.fallback?.line).toBe("Studio Mac mini, your default, is offline. This runs on Dillon's PC, your next online computer. If none is online, it runs on This Mac.");
    expect(d.fallback?.steps.map((s) => [s.name, s.note])).toEqual([
      ["Studio Mac mini", "Your default. Offline now"],
      ["Dillon's PC", "Next online computer. Used now"],
      ["This Mac", "Last resort"],
    ]);
  });

  it("runs on This Mac when nothing else is online, and never makes a new computer", () => {
    const t = tree({ defaultSpace: "relay:studio", machines: [THIS_MAC, { ...STUDIO, online: false }, { ...DILLON, online: false }] });
    const d = treeDefault(t, settings("relay:studio"), null);
    expect(d.where).toEqual({ kind: "this-mac" });
    expect(d.fallback?.line).toBe("Studio Mac mini, your default, is offline, and no other computer is online. This runs on This Mac.");
    expect(d.fallback?.steps.at(-1)).toMatchObject({ name: "This Mac", note: "Last resort. Used now", used: true });
    // Fallback off: only the default, then This Mac.
    const off = treeDefault(tree({ defaultSpace: "relay:studio", machines: [THIS_MAC, { ...STUDIO, online: false }, DILLON] }), settings("relay:studio", [], false), null);
    expect(off.where).toEqual({ kind: "this-mac" });
  });

  it("follows the order to try, and the relay's own answer when it gave one", () => {
    const machines = [THIS_MAC, { ...STUDIO, online: false }, DILLON, machine({ id: "linux", name: "Linux box", os: "linux" })];
    const spaces = [...SPACES, space({ id: "relay:linux", name: "Linux box" })];
    const t = tree({ defaultSpace: "relay:studio", machines, spaces });
    expect(treeDefault(t, settings("relay:studio", ["relay:linux", "relay:dillon"]), null).where).toMatchObject({ kind: "space", space: { id: "relay:linux" } });
    const relay = { space: "relay:dillon", reason: "fallback", defaultSpace: "relay:studio", defaultOnline: false, order: [{ space: "relay:studio", name: "Studio Mac mini", online: false, role: "default" }, { space: "relay:dillon", name: "Dillon's PC", online: true, role: "fallback" }, { space: "relay:mac-here", name: "This Mac", online: true, role: "here" }] };
    expect(treeDefault(t, settings("relay:studio"), relay).where).toMatchObject({ kind: "space", space: { id: "relay:dillon" } });
    // The relay says "here" with no Space: This Mac.
    expect(treeDefault(t, settings("relay:studio"), { ...relay, space: null, reason: "here" }).where).toEqual({ kind: "this-mac" });
  });

  it("with no default set, uses the first other computer, else This Mac", () => {
    expect(treeDefault(tree(), null, null).where).toMatchObject({ kind: "space", space: { id: "relay:studio" } });
    const single = buildSpacesTree({ spaces: [SPACES[0]!], machines: [THIS_MAC], defaultSpace: null, now: NOW });
    expect(treeDefault(single, null, null).where).toEqual({ kind: "this-mac" });
  });

  it("lists This Mac last among the candidates", () => {
    const c = defaultCandidates(tree());
    expect(c.map((x) => x.space)).toEqual(["relay:mac-here", "relay:studio", "relay:dillon"]);
    expect(c[0]!.here).toBe(true);
  });
});

describe("currentTreeWhere", () => {
  const t = tree({ defaultSpace: "relay:studio" });
  const fallback = { kind: "space", space: SPACES[2]! } as const;
  const kinds = [{ os: "linux" as const, note: "", blocked: null }];

  it("keeps a pick that's still usable", () => {
    expect(currentTreeWhere({ kind: "space", space: SPACES[6]! }, fallback, kinds, t)).toEqual({ kind: "space", space: SPACES[6] });
    expect(currentTreeWhere({ kind: "this-mac" }, fallback, kinds, t)).toEqual({ kind: "this-mac" });
    expect(currentTreeWhere({ kind: "new", os: "linux", on: { machine: "dillon", name: "Dillon's PC" } }, fallback, kinds, t)).toMatchObject({ kind: "new", on: { machine: "dillon" } });
  });

  it("falls back to the default, never to a new computer, when the pick went away or went offline", () => {
    expect(currentTreeWhere({ kind: "space", space: space({ id: "relay:gone" }) }, fallback, kinds, t)).toBe(fallback);
    const off = tree({ machines: [THIS_MAC, STUDIO, { ...DILLON, online: false }] });
    expect(currentTreeWhere({ kind: "space", space: SPACES[6]! }, fallback, kinds, off)).toBe(fallback);
    expect(currentTreeWhere({ kind: "new", os: "linux", on: { machine: "dillon", name: "Dillon's PC" } }, fallback, kinds, off)).toBe(fallback);
    expect(currentTreeWhere(null, fallback, kinds, t)).toBe(fallback);
  });

  it("offers New Space on This Mac and on each online computer", () => {
    expect(newSpaceTargets(t).map((n) => [n.name, n.on?.machine ?? null])).toEqual([
      ["This Mac", null],
      ["Studio Mac mini", "studio"],
      ["Dillon's PC", "dillon"],
    ]);
  });

  it("finds a row by its key or the relay's id", () => {
    expect(findNode(t, "relay:studio/build-box")?.name).toBe("Build box");
    expect(findNode(t, "relay:mac-here")?.name).toBe("This Mac");
  });
});

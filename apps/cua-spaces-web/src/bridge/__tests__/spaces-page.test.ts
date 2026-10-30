// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { replayFlow } from "../parity";
import {
  accountMenu,
  bulkConfirm,
  bulkDone,
  bulkProgress,
  chooseSettings,
  proBanner,
  spaceMenu,
  tipsReduce,
  tipsView,
} from "../spaces-page";
import type { BulkItem, TipsState } from "../contracts/spaces-page";
import { testCore, wasmBuilt } from "./testCore";

const GB = 1_000_000_000;
const NONE: TipsState = { off: false, done: [] };

const mine = (name: string, gb: number | null, extra: Partial<BulkItem> = {}): BulkItem => ({
  id: `local:${name}`,
  name,
  os: "linux",
  bytes: gb == null ? null : gb * GB,
  mine: true,
  place: "This Mac",
  ...extra,
});

describe.skipIf(!wasmBuilt)("the new Spaces page's decisions (wasm core)", () => {
  it("menus show only what you may do", async () => {
    const core = await testCore();
    const labels = (input: Parameters<typeof spaceMenu>[1]) => spaceMenu(core, input).map((i) => `${i.separatorBefore ? "--- " : ""}${i.label}${i.enabled ? "" : " (disabled)"}${i.destructive ? " (red)" : ""}`);
    const all = { canPower: true, canTeleport: true, canShare: true, canRename: true, canDelete: true };
    expect(labels({ kind: "mine", power: "running", shared: true, ...all })).toEqual([
      "Open",
      "Turn off",
      "Manage sharing…",
      "Teleport an app…",
      "Rename",
      "--- Delete Space (red)",
    ]);
    expect(labels({ kind: "withMe", role: "editor", power: "off", canPower: true, canLeave: true })).toEqual(["Open", "Turn on", "--- Leave"]);
    expect(labels({ kind: "team", power: "off", canPower: true, canTeleport: true, canLeave: true })).toEqual([
      "Open",
      "Turn on",
      "Teleport an app… (disabled)",
      "--- Leave",
    ]);
    expect(spaceMenu(core, { kind: "mine", ...all }).at(-1)?.id).toBe("delete");
  });

  it("Pin to notch / Unpin from notch sit above Delete and Leave, for a Space and for another machine's desktop", async () => {
    const core = await testCore();
    const labels = (input: Parameters<typeof spaceMenu>[1]) => spaceMenu(core, input).map((i) => `${i.separatorBefore ? "--- " : ""}${i.label}`);
    const all = { canPower: true, canTeleport: true, canShare: true, canDelete: true };
    expect(labels({ kind: "mine", power: "running", canPin: true, ...all })).toEqual(["Open", "Turn off", "Share…", "Teleport an app…", "Pin to notch", "--- Delete Space"]);
    expect(labels({ kind: "mine", power: "running", canPin: true, pinned: true, ...all }).at(-2)).toBe("Unpin from notch");
    expect(labels({ kind: "withMe", role: "viewer", power: "running", canPin: true, canLeave: true })).toEqual(["Open", "Pin to notch", "--- Leave"]);
    // A machine's desktop: Open and the pin, nothing to turn on, share or delete.
    expect(labels({ kind: "mine", role: "owner", power: "off", canPin: true })).toEqual(["Open", "Pin to notch"]);
    // Where the host can't keep pins (or for This Mac) there is no item.
    expect(labels({ kind: "mine", role: "owner", power: "off" })).toEqual(["Open"]);
    // The pins' order is the core's: pinned last, unpinned when pinned, never This Mac.
    expect(core.call<string[]>("notch.togglePin", { pinned: ["a"], id: "b" })).toEqual(["a", "b"]);
    expect(core.call<string[]>("notch.togglePin", { pinned: ["a", "b"], id: "a" })).toEqual(["b"]);
    expect(core.call<string[]>("notch.togglePin", { pinned: [], id: "this-mac" })).toEqual([]);
  });

  it("asks once for a mixed selection and counts what is freed", async () => {
    const core = await testCore();
    const items: BulkItem[] = [
      mine("cua-builder", 14, { running: true, people: [{ who: "sam@example.com", name: "Sam" }] }),
      { id: "relay:m/qa-shared", name: "qa-shared", os: "linux", mine: false, place: "Sam's Mac mini" },
      mine("onb-new", 9),
    ];
    const view = bulkConfirm(core, items)!;
    expect(view.title).toBe("Delete 2 Spaces and leave 1?");
    expect(view.body).toBe(
      "cua-builder is running and will be turned off first. Sam loses access. Their files and apps are gone for good. qa-shared stays on Sam's Mac mini.",
    );
    expect(view.groups.map((g) => [g.title, g.rows.map((r) => r.detail)])).toEqual([
      ["Delete", ["14 GB", "9 GB"]],
      ["Leave", ["Sam's Mac mini"]],
    ]);
    expect([view.freesLabel, view.frees, view.button, view.destructive]).toEqual(["Frees", "23 GB", "Delete and leave", true]);
    expect(view.deleteIds).toEqual(["local:cua-builder", "local:onb-new"]);
    expect(view.leaveIds).toEqual(["relay:m/qa-shared"]);
    expect(bulkConfirm(core, [])).toBeNull();

    expect(bulkProgress(core, items, [])).toBe("Deleting 1 of 2…");
    expect(bulkProgress(core, items, ["local:cua-builder", "local:onb-new"])).toBe("Leaving 1 of 1…");
    expect(bulkProgress(core, items, items.map((i) => i.id))).toBeNull();

    const done = bulkDone(core, items, items.map((i) => ({ id: i.id, ok: true })));
    expect(done).toEqual({ text: "Deleted 2 Spaces and left 1. 23 GB freed.", tone: "ok" });
    const partial = bulkDone(core, [items[0]!, items[2]!], [
      { id: "local:cua-builder", ok: true },
      { id: "local:onb-new", ok: false, error: "it is busy" },
    ]);
    expect(partial).toEqual({ text: "Deleted 1 of 2 Spaces. 14 GB freed. Couldn't delete onb-new: it is busy.", tone: "partial" });
  });

  it("offers one tip at a time, Hermes before Claude Code", async () => {
    const core = await testCore();
    const facts = { newestRunning: "cua-builder", agentsConnected: ["claude-code", "hermes"], hasStopped: true, secondComputerOnline: true };
    const tip = tipsView(core, "spaces", facts, NONE)!;
    expect(tip.text).toBe("Try cua-builder in Hermes");
    expect(tip.action).toEqual({
      kind: "copy",
      label: "Copy",
      value: 'Open the Cua Space "cua-builder" and tell me what\'s on its screen.',
      doneLabel: "Copied",
    });
    expect(tip.openIn).toBeNull();
    expect(tipsView(core, "spaces", { ...facts, agentsConnected: ["cursor"] }, NONE)?.openIn).toEqual({
      label: "Open in Cursor",
      url: "cursor://anysphere.cursor-deeplink/prompt?text=Open%20the%20Cua%20Space%20%22cua-builder%22%20and%20tell%20me%20what%27s%20on%20its%20screen.",
    });
    const next = tipsReduce(core, NONE, { type: "next", id: tip.id });
    expect(next).toEqual({ off: false, done: ["try-agent"] });
    expect(tipsView(core, "spaces", facts, next)?.id).toBe("power");
    // The sidebar's tip waits for a page's.
    expect(tipsView(core, "sidebar", { ...facts, pageTipShown: true }, next)).toBeNull();
    expect(tipsView(core, "sidebar", facts, next)?.action).toMatchObject({ kind: "copyCommand", value: "cua create --on auto" });
    const off = tipsReduce(core, next, { type: "dont-show" });
    expect(tipsView(core, "spaces", facts, off)).toBeNull();
  });

  it("applies Settings' Show tips and Reset tips", async () => {
    const core = await testCore();
    const off = chooseSettings(core, { tips: { off: false, done: ["power"] } }, "tips", "off");
    expect(off.tips).toEqual({ off: true, done: ["power"] });
    expect(chooseSettings(core, off, "tips", "on").tips).toEqual({ off: false, done: ["power"] });
    expect(chooseSettings(core, off, "reset-tips", "press").tips).toEqual(NONE);
  });

  it("shows the Pro banner on Free only", async () => {
    const core = await testCore();
    expect(proBanner(core, { plan: "free" })).toEqual({ visible: true, text: "Run more Spaces at once with Pro", button: "Upgrade" });
    for (const plan of ["pro", "team", "teams", ""]) expect(proBanner(core, { plan }).visible).toBe(false);
  });

  it("words the account menu", async () => {
    const core = await testCore();
    expect(accountMenu(core, { plan: "free", signedIn: true, authMode: "user" })).toEqual([
      { id: "settings", label: "Settings", value: null, separatorBefore: false },
      { id: "plan", label: "Plan", value: "Free", separatorBefore: false },
      { id: "signOut", label: "Sign out", value: null, separatorBefore: true },
    ]);
    expect(accountMenu(core, { plan: "team", signedIn: true, authMode: "client-credentials" }).map((i) => i.value ?? i.label)).toEqual(["Settings", "Teams"]);
  });

  it("replays the new parity flows through these wrappers", async () => {
    const core = await testCore();
    for (const name of ["spaces-menu", "bulk-confirm", "tips", "pro-banner"]) {
      const replay = replayFlow(core, name);
      expect(replay.bridged.length, name).toBeGreaterThan(0);
      expect(replay.transcript, name).toEqual(replay.golden);
    }
  });
});

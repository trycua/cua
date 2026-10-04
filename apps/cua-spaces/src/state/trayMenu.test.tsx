// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { FIXTURE_NOW, FIXTURE_SPACES } from "../model/fixtures";
import { withThisMachine } from "../model/host";
import { notchTab } from "../model/notch";
import { openableCount, trayMenu, type MenuItem } from "../model/window";
import { TRAY_SYNC_POLL_MS, useTrayMenu } from "./trayMenu";

// Cua Volume's sync line shows with its experiment on (Settings,
// Experiments); "hides Cua Volume's sync ..." turns it off.
beforeEach(() => {
  window.localStorage.setItem("cua.settings.experiments", JSON.stringify({ cuaVolume: true }));
});

/** The notch's roster: "This machine" (not shared) and the Spaces. */
const ROSTER = withThisMachine(FIXTURE_SPACES, null, FIXTURE_NOW, "macos");

function sync(over: Record<string, unknown> = {}) {
  return {
    device_id: "d1",
    device_name: "maya-mbp",
    feed: "live",
    last_poll_ms: FIXTURE_NOW,
    pending_uploads: 0,
    conflicts: [],
    devices: [],
    last_error: null,
    ...over,
  };
}

const tick = (ms: number) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });

const conflict = { path: "public/plan.md", conflict_path: "public/plan (conflict).md", winner_device: "d1", loser_device: "d2", ts_ms: 0 };

const S3 = { backend: "s3", fs_path: "/Users/maya/.cua/volume/data", s3: null, has_keys: true };
const FS = { ...S3, backend: "fs", has_keys: false };

/** `answer` is `volume_sync_status`'s; `storage` is `volume_storage`'s (a bucket by default). */
function run(answer: () => unknown, now: () => number = () => FIXTURE_NOW, storage: () => unknown = () => S3) {
  const calls: string[] = [];
  const call = vi.fn(async (tool: string) => {
    calls.push(tool);
    const a = tool === "volume_storage" ? storage() : answer();
    if (a instanceof Error) throw a;
    return a;
  });
  const menus: MenuItem[][] = [];
  const setMenu = vi.fn(async (items: MenuItem[]) => {
    menus.push(items);
  });
  const hook = renderHook(({ spaces }) => useTrayMenu(spaces, { call, setMenu, now }), { initialProps: { spaces: ROSTER } });
  return { calls, menus, setMenu, hook };
}

describe("the menu bar item's menu", () => {
  it("is the core's menu for the notch's roster: the same count as the notch tab", async () => {
    const { menus, calls } = run(() => new Error("unknown tool volume_sync_status"));
    await waitFor(() => expect(calls).toEqual(["volume_sync_status", "volume_storage"]));
    const items = menus.at(-1)!;
    expect(items).toEqual(trayMenu({ spaces: ROSTER, keyvault: null, sync: null, backend: "s3", nowMs: FIXTURE_NOW }));
    // "This machine" (not shared) is in the roster but not in the count.
    expect(ROSTER.some((s) => s.id === "this-mac")).toBe(true);
    const n = openableCount(ROSTER);
    expect(n).toBe(ROSTER.length - 1);
    expect(String(notchTab(ROSTER).count)).toBe(String(n));
    expect(items[0]).toMatchObject({ id: "status", label: `${n} Spaces`, enabled: false });
    expect(items.map((i) => i.id)).not.toContain("volume-conflicts");
  });

  it("hides Cua Volume's sync and conflicts without the Cua Volume experiment", async () => {
    window.localStorage.removeItem("cua.settings.experiments");
    vi.useFakeTimers();
    try {
      const { menus } = run(() => sync({ pending_uploads: 3, conflicts: [conflict] }));
      await tick(0);
      const items = menus.at(-1)!;
      expect(items[0]!.label).toBe(`${openableCount(ROSTER)} Spaces`);
      expect(items.map((i) => i.id)).not.toContain("volume-conflicts");
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows Cua Volume's sync on the status line and its conflicts under it", async () => {
    let answer: unknown = sync({ pending_uploads: 3, conflicts: [conflict, conflict] });
    vi.useFakeTimers();
    try {
      const { menus, setMenu } = run(() => answer);
      await tick(0);
      const n = openableCount(ROSTER);
      let items = menus.at(-1)!;
      expect(items[0]!.label).toBe(`${n} Spaces · Syncing 3…`);
      expect(items[1]).toEqual({ id: "volume-conflicts", label: "2 conflicts", shortcut: null, enabled: true });

      answer = sync();
      await tick(TRAY_SYNC_POLL_MS);
      items = menus.at(-1)!;
      expect(items[0]!.label).toBe(`${n} Spaces · Synced`);
      expect(items.map((i) => i.id)).not.toContain("volume-conflicts");

      answer = sync({ feed: "error" });
      await tick(TRAY_SYNC_POLL_MS);
      expect(menus.at(-1)![0]!.label).toBe(`${n} Spaces · Offline`);

      // This machine's store, one device: no sync word.
      answer = sync({ feed: "off", conflicts: [conflict] });
      await tick(TRAY_SYNC_POLL_MS);
      expect(menus.at(-1)![0]!.label).toBe(`${n} Spaces`);
      expect(menus.at(-1)!.map((i) => i.id)).not.toContain("volume-conflicts");

      // Unchanged reads send nothing.
      const sent = setMenu.mock.calls.length;
      await tick(TRAY_SYNC_POLL_MS * 2);
      expect(setMenu.mock.calls.length).toBe(sent);
    } finally {
      vi.useRealTimers();
    }
  });

  it("says Offline once the bucket has not answered for a while, though the feed is live", async () => {
    let clock = FIXTURE_NOW + 1_000;
    vi.useFakeTimers();
    try {
      const { menus } = run(() => sync(), () => clock);
      await tick(0);
      const n = openableCount(ROSTER);
      expect(menus.at(-1)![0]!.label).toBe(`${n} Spaces \u00b7 Synced`);
      // The same answer (last poll unchanged), 31 s later.
      clock = FIXTURE_NOW + 31_000;
      await tick(TRAY_SYNC_POLL_MS);
      expect(menus.at(-1)![0]!.label).toBe(`${n} Spaces \u00b7 Offline`);
      expect(menus.at(-1)).toEqual(
        trayMenu({ spaces: ROSTER, keyvault: null, sync: sync() as never, backend: "s3", nowMs: clock }),
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("has no sync word on this Mac's store with one device; a bucket has one", async () => {
    const one = () =>
      sync({
        pending_uploads: 2,
        conflicts: [conflict],
        devices: [{ id: "d1", name: "maya-mbp", this_device: true, last_seen_ms: FIXTURE_NOW, last_change_ms: FIXTURE_NOW }],
      });
    let storage: unknown = FS;
    vi.useFakeTimers();
    try {
      const { menus, calls } = run(one, () => FIXTURE_NOW, () => storage);
      await tick(0);
      const n = openableCount(ROSTER);
      // The engine says "live" even here; the core leaves it out.
      expect(menus.at(-1)![0]!.label).toBe(`${n} Spaces`);
      expect(menus.at(-1)!.map((i) => i.id)).not.toContain("volume-conflicts");
      storage = S3;
      await tick(TRAY_SYNC_POLL_MS);
      expect(menus.at(-1)![0]!.label).toBe(`${n} Spaces \u00b7 Syncing 2\u2026`);
      expect(menus.at(-1)!.map((i) => i.id)).toContain("volume-conflicts");
      // Both are read on every tick.
      expect(calls.filter((c) => c === "volume_storage")).toHaveLength(2);
      expect(calls.filter((c) => c === "volume_sync_status")).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("follows the roster", async () => {
    const { menus, hook, calls } = run(() => null);
    await waitFor(() => expect(calls.length).toBe(2));
    const fewer = ROSTER.filter((s) => s.id !== "this-mac" && s.status !== "local").slice(0, 1);
    hook.rerender({ spaces: fewer });
    await waitFor(() => expect(menus.at(-1)![0]!.label).toBe("1 Space"));
    expect(menus.at(-1)).toEqual(trayMenu({ spaces: fewer, keyvault: null, sync: null, backend: "s3", nowMs: FIXTURE_NOW }));
  });
});

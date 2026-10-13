// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space detail's models without the native layer: the thumbnail store
// (the SwiftUI app's SpaceThumbnails, its tests' expectations), the
// picture-in-picture set (libs/spaces-app-swift's PiPTests) and the drop
// well's offered paths (WebUIBridge+Files.swift).
import { describe, expect, it } from "vitest";
import { dropped, FileOffers } from "../src/bridge/files";
import { NotchActivity } from "../src/model/activity";
import { isAppBundle } from "../src/notch-feed";
import { LiveSpacesBackend, type SpaceThumbnailData } from "../src/model/backend";
import type { Native } from "../src/native/load";
import { DESKTOP, PipSet, pipKey, pipTitle, remoteWindow, type PipSpec, type StreamWindow } from "../src/model/streams";
import { SpaceThumbnails, thumbnailDataUrl } from "../src/model/thumbnails";

const policy = { openIntervalMs: 15_000, backgroundIntervalMs: 90_000, maxDimension: 320 };
const thumb = (capturedAtMs: number, side = 10, byte = 1): SpaceThumbnailData => ({
  image: Uint8Array.of(0xff, 0xd8, byte),
  format: "jpeg",
  width: side,
  height: side,
  capturedAtMs,
});

describe("SpaceThumbnails", () => {
  it("keeps the newest image per Space", () => {
    const t = new SpaceThumbnails(policy);
    t.set("a", thumb(200, 10, 2));
    t.set("a", thumb(100, 10, 1));
    expect(t.get("a")?.capturedAtMs).toBe(200);
    t.set("a", thumb(300, 10, 3));
    expect(t.get("a")?.image[2]).toBe(3);
    expect(t.totalBytes).toBe(400);
  });

  it("drops the least recently used images past the byte cap, never the one just set", () => {
    const t = new SpaceThumbnails(policy);
    t.byteCap = 1000;
    t.set("a", thumb(1)); // 400 bytes
    t.set("b", thumb(1)); // 800
    t.get("a"); // a is now the most recent
    t.set("c", thumb(1)); // 1200: b goes
    expect([t.get("a"), t.get("b"), t.get("c")].map((e) => e !== null)).toEqual([true, false, true]);
    expect(t.totalBytes).toBe(800);
    t.byteCap = 100;
    t.set("d", thumb(1, 20)); // 1600 bytes alone: kept
    expect(t.size).toBe(1);
    expect(t.get("d")).not.toBeNull();
  });

  it("forgets Spaces that left the list", () => {
    const t = new SpaceThumbnails(policy);
    t.set("a", thumb(1));
    t.set("b", thumb(1));
    t.retain(new Set(["b"]));
    expect(t.get("a")).toBeNull();
    expect(t.totalBytes).toBe(400);
  });

  it("asks the SDK once per Space at a time, with the age in milliseconds", async () => {
    const asks: [string, number | null][] = [];
    let release: (v: SpaceThumbnailData) => void = () => {};
    const t = new SpaceThumbnails(policy, (id, maxAgeMs) => {
      asks.push([id, maxAgeMs]);
      return new Promise((r) => (release = r));
    });
    const first = t.refresh("a", 15);
    const second = t.refresh("a", 15);
    release(thumb(5));
    expect((await first)?.capturedAtMs).toBe(5);
    expect((await second)?.capturedAtMs).toBe(5);
    expect(asks).toEqual([["a", 15_000]]);
  });

  it("keeps the image it has when a refresh fails", async () => {
    const t = new SpaceThumbnails(policy, async () => {
      throw new Error("offline");
    });
    t.set("a", thumb(5));
    expect((await t.refresh("a", null))?.capturedAtMs).toBe(5);
  });

  it("warms each Space without an image once, from the cache at any age", async () => {
    const asks: [string, number | null][] = [];
    const t = new SpaceThumbnails(policy, async (id, maxAgeMs) => {
      asks.push([id, maxAgeMs]);
      return null;
    });
    t.set("held", thumb(1));
    await t.warm(["held", "a", "b"]);
    await t.warm(["a", "b"]);
    expect(asks).toEqual([
      ["a", null],
      ["b", null],
    ]);
  });

  it("keeps running Spaces fresh at the background interval while the app is in use", async () => {
    const asks: [string, number | null][] = [];
    const t = new SpaceThumbnails(policy, async (id, maxAgeMs) => {
      asks.push([id, maxAgeMs]);
      return null;
    });
    let active = false;
    const sleeps: number[] = [];
    let wake: () => void = () => {};
    const stop = t.keepFresh(
      () => ["a", "b"],
      () => active,
      (ms) => {
        sleeps.push(ms);
        return new Promise((r) => (wake = r));
      },
    );
    await new Promise((r) => setTimeout(r, 0));
    expect(asks).toEqual([]);
    active = true;
    wake();
    await new Promise((r) => setTimeout(r, 0));
    expect(asks).toEqual([
      ["a", 90_000],
      ["b", 90_000],
    ]);
    expect(sleeps).toEqual([90_000, 90_000]);
    stop();
    wake();
  });

  it("tells listeners which Space's image changed", () => {
    const t = new SpaceThumbnails(policy);
    const changed: string[] = [];
    t.subscribe((id) => changed.push(id));
    t.set("a", thumb(1));
    t.remove("a");
    t.remove("a");
    expect(changed).toEqual(["a", "a"]);
  });

  it("answers an image as a data URL of its own type", () => {
    expect(thumbnailDataUrl({ image: Uint8Array.of(1, 2, 3), format: "png" })).toBe("data:image/png;base64,AQID");
  });
});

const terminal: StreamWindow = { id: "w-7", app: "Terminal", title: "cua@space: ~", epoch: 3, width: 0, height: 0, appId: "", pid: 0 };

describe("picture in picture", () => {
  it("keys one panel per desktop or window handle, whatever its epoch or size", () => {
    expect(pipKey(DESKTOP)).toBe("desktop");
    expect(pipKey({ kind: "window", window: terminal })).toBe("window:w-7");
    const later = { ...terminal, epoch: 9, width: 640, height: 480 };
    expect(pipKey({ kind: "window", window: later })).toBe(pipKey({ kind: "window", window: terminal }));
    expect(pipTitle(DESKTOP)).toBe("Desktop");
    expect(pipTitle({ kind: "window", window: terminal })).toBe("cua@space: ~");
    expect(pipTitle({ kind: "window", window: { ...terminal, id: "w-8", app: "Thunar", title: "" } })).toBe("Thunar");
  });

  function presenter() {
    const open = new Map<string, () => void>();
    const specs: PipSpec[] = [];
    const closed: string[] = [];
    return {
      open,
      specs,
      closed,
      p: {
        open: (spec: PipSpec, gone: () => void) => {
          specs.push(spec);
          open.set(spec.key, gone);
        },
        close: (_id: string, key: string) => {
          closed.push(key);
          open.delete(key);
        },
      },
    };
  }

  it("opens a window and the desktop beside it, and closes each on its own", () => {
    const { p, specs, closed } = presenter();
    const pips = new PipSet("local:a", () => ({ name: "Aurora", os: "linux" }), p);
    pips.popOut({ kind: "window", window: terminal });
    pips.popOut(DESKTOP);
    expect([...pips.openKeys]).toEqual(["window:w-7", "desktop"]);
    expect(specs.map((s) => [s.spaceId, s.spaceName, s.os, s.key, s.title])).toEqual([
      ["local:a", "Aurora", "linux", "window:w-7", "cua@space: ~"],
      ["local:a", "Aurora", "linux", "desktop", "Desktop"],
    ]);
    pips.popIn(DESKTOP);
    pips.popIn(DESKTOP);
    expect(closed).toEqual(["desktop"]);
    expect(pips.isOpen({ kind: "window", window: terminal })).toBe(true);
    pips.popInAll();
    expect(pips.openKeys.size).toBe(0);
    expect(closed).toEqual(["desktop", "window:w-7"]);
  });

  it("closing a panel with its own button is popping it in", () => {
    const { p, open } = presenter();
    const pips = new PipSet("local:a", () => ({ name: "Aurora", os: "linux" }), p);
    pips.popOut({ kind: "window", window: terminal });
    open.get("window:w-7")!();
    expect(pips.isOpen({ kind: "window", window: terminal })).toBe(false);
  });

  it("describes a window as the core's remote window", () => {
    expect(remoteWindow({ ...terminal, width: 1279.6, height: 800, appId: "org.gnome.Terminal", pid: 412 })).toEqual({
      id: "w-7",
      appName: "Terminal",
      title: "cua@space: ~",
      visible: true,
      appId: "org.gnome.Terminal",
      targetEpoch: 3,
      widthPx: 1280,
      heightPx: 800,
      pid: 412,
    });
    expect(remoteWindow(terminal)).toMatchObject({ widthPx: null, heightPx: null, pid: null });
  });
});

describe("the drop well's paths", () => {
  it("offers only the drop's files whose names the page saw dropped", () => {
    const paths = ["/Users/ada/notes.txt", "/Users/ada/Photos/cat.png", "/Users/ada/Apps/Notes.app"];
    expect(dropped(paths, ["notes.txt", "Notes.app"])).toEqual(["/Users/ada/notes.txt", "/Users/ada/Apps/Notes.app"]);
    expect(dropped(paths, [])).toEqual([]);
    expect(dropped([], ["notes.txt"])).toEqual([]);
  });

  it("remembers what was offered, and nothing else", () => {
    const o = new FileOffers();
    expect(o.offer(["/a", "", "/b"])).toEqual(["/a", "/b"]);
    expect(o.has("/a")).toBe(true);
    expect(o.has("/c")).toBe(false);
  });
});

describe("the notch's activity", () => {
  it("shows a transfer until the last of overlapping ones ends, once each", async () => {
    const a = new NotchActivity();
    const seen: boolean[] = [];
    a.subscribe(() => seen.push(a.transfer));
    const first = a.begin();
    const second = a.begin();
    first();
    first();
    expect(a.transfer).toBe(true);
    second();
    expect(seen).toEqual([true, false]);
    await expect(a.during(async () => Promise.reject(new Error("x")))).rejects.toThrow("x");
    expect(a.transfer).toBe(false);
    a.setHotspot(true);
    a.setHotspot(true);
    expect(seen).toEqual([true, false, true, false, false]);
  });

  it("tells app bundles from files", () => {
    expect(["/Applications/Notes.app", "/A/Notes.app/", "/a/app.txt", "/a/notes.APP"].map(isAppBundle)).toEqual([true, true, false, true]);
  });
});

describe("LiveSpacesBackend's runtime settings", () => {
  // The SDK's `runtime.*` config, without the native layer.
  const native = { configGet: (key: string) => ({ value: key === "runtime.lume" ? "builtin" : "auto" }) } as unknown as Native;
  const backend = (platform: NodeJS.Platform) => new LiveSpacesBackend(native, {} as never, {}, fetch, platform);

  it("has a macOS VMs (Lume) setting on a Mac only", async () => {
    expect(await backend("darwin").lumeSource()).toBe("builtin");
    // Windows and Linux: no Lume, so Settings shows no macOS VMs row.
    expect(await backend("linux").lumeSource()).toBeNull();
    expect(await backend("win32").lumeSource()).toBeNull();
    expect(await backend("linux").linuxSource()).toBe("auto");
  });
});

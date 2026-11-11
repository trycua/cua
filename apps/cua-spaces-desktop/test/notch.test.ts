// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import { createRequire } from "node:module";
import * as path from "node:path";
import { PassThrough } from "node:stream";
import { describe, expect, it, vi } from "vitest";
import { helperPath, notchSupported, startNotch, type NotchActions, type WindowDragEvent, type WindowTargets } from "../src/notch";
import { followDragReadiness, notchTeleport } from "../src/notch-feed";
import { fakeCore, manualClock, MOTION } from "./fakeNotchCore";

const require = createRequire(import.meta.url);
const config = require("../electron-builder.config.cjs");
const pkg = require("../package.json");

describe("where the notch runs", () => {
  it("runs on macOS 26 and later only (the SwiftUI app's minimum)", () => {
    expect(notchSupported("darwin", "25.0.0")).toBe(true);
    expect(notchSupported("darwin", "26.1.0")).toBe(true);
    expect(notchSupported("darwin", "24.6.0")).toBe(false);
    expect(notchSupported("win32", "10.0.26100")).toBe(false);
    expect(notchSupported("linux", "6.8.0-45-generic")).toBe(false);
  });

  it("finds the helper in Contents/Helpers when packaged and in native/notch in development", () => {
    const resources = "/Applications/Cua Spaces.app/Contents/Resources";
    // path.join's separators, so the test reads the same on a Windows runner.
    expect(helperPath({ packaged: true, resourcesPath: resources, appPath: `${resources}/app.asar` })).toBe(
      path.join("/Applications/Cua Spaces.app/Contents/Helpers/Cua Spaces Notch.app/Contents/MacOS/Cua Spaces Notch"),
    );
    expect(helperPath({ packaged: false, resourcesPath: "/x", appPath: "/src/apps/cua-spaces-desktop" })).toBe(
      path.join("/src/apps/cua-spaces-desktop/native/notch/Cua Spaces Notch.app/Contents/MacOS/Cua Spaces Notch"),
    );
  });
});

describe("packaging the notch", () => {
  it("builds the helper with pnpm notch and ships it in Contents/Helpers when built", () => {
    expect(pkg.scripts.notch).toBe("node scripts/build-notch.mjs");
    const files = config.mac.extraFiles as { from: string; to: string }[];
    for (const f of files) {
      expect(f.from.endsWith(path.join("native", "notch", "Cua Spaces Notch.app"))).toBe(true);
      expect(f.to).toBe("Helpers/Cua Spaces Notch.app");
    }
  });
});

class FakeChild extends EventEmitter {
  stdin = new PassThrough();
  stdout = new PassThrough();
  stderr = new PassThrough();
  exitCode: number | null = null;
  signalCode: NodeJS.Signals | null = null;
  written = "";
  constructor() {
    super();
    this.stdin.setEncoding("utf8");
    this.stdin.on("data", (c: string) => (this.written += c));
  }
  get received(): { type: string; [k: string]: unknown }[] {
    return this.written.split("\n").filter(Boolean).map((l) => JSON.parse(l));
  }
  say(m: unknown) {
    this.stdout.write(`${JSON.stringify(m)}\n`);
  }
  kill() {
    return true;
  }
}

function setup(trace?: (line: string) => void, drags?: { windowTargets: WindowTargets; onListener(l: { onEvent(e: WindowDragEvent): void }): void }) {
  const fake = fakeCore();
  const clock = manualClock();
  const children: FakeChild[] = [];
  const calls: unknown[][] = [];
  const record =
    (name: string) =>
    (...args: unknown[]) =>
      void calls.push([name, ...args]);
  const actions: NotchActions = {
    openSpace: record("openSpace"),
    openMain: record("openMain"),
    openSettings: record("openSettings"),
    openAccess: record("openAccess"),
    dismissAccess: record("dismissAccess"),
    teleport: record("teleport"),
    drop: record("drop"),
  };
  const opened: string[] = [];
  const requested: number[] = [];
  const notch = startNotch(
    {
      core: fake.core,
      actions,
      thumbnail: async () => new Uint8Array([0x89, 0x50, 0x4e, 0x47]),
      teleport: {
        windowDragSupported: () => true,
        windowDragPermitted: () => !!drags,
        requestWindowDragPermission: () => (requested.push(1), true),
        startWindowDrag: (l) => (drags?.onListener(l), { stop() {} }),
        captureWindowThumbnail: () => null,
      },
      windowTargets: drags?.windowTargets,
    },
    {
      path: "/helper",
      trace,
      clock,
      openURL: (u) => opened.push(u),
      log: () => {},
      windowUnderTimeoutMs: 50,
      process: {
        spawn: () => {
          const c = new FakeChild();
          children.push(c);
          return c as unknown as ChildProcess;
        },
        after: clock.after,
        now: clock.now,
      },
    },
  );
  notch.setSpaces([{ id: "local:aurora", name: "aurora" }]);
  return { notch, fake, clock, children, calls, opened, requested };
}

const SCREEN = {
  frame: { x: 0, y: 0, width: 1512, height: 982 },
  visibleFrame: { x: 0, y: 0, width: 1512, height: 945 },
  safeAreaTop: 32,
};

describe("the notch end to end (fake helper)", () => {
  it("greets the helper with the core's motion, then sends the state with a layout once it reports its screens", async () => {
    const { children } = setup();
    const c = children[0]!;
    await vi.waitFor(() => expect(c.received.map((m) => m.type)).toEqual(["hello", "state", "state"]));
    expect(c.received[0]).toEqual({ type: "hello", v: 1, motion: MOTION, radii: { closed: { top: 6, bottom: 14 }, open: { top: 19, bottom: 24 } } });
    expect(c.received.at(-1)?.layout).toBeUndefined();
    c.say({ type: "hello", v: 1, pid: 4242 });
    c.say({ type: "screens", notch: SCREEN, primary: SCREEN });
    await vi.waitFor(() => expect(c.received.at(-1)?.layout).toMatchObject({ notchStyle: true }));
  });

  it("runs the helper's input through the core and its clicks to the app", async () => {
    const { children, calls, opened, requested, fake } = setup();
    const c = children[0]!;
    c.say({ type: "event", event: { kind: "click" } });
    await vi.waitFor(() => expect((c.received.at(-1)?.view as { phase: string }).phase).toBe("tiles"));
    c.say({ type: "action", action: "openSpace", spaceId: "local:aurora" });
    c.say({ type: "action", action: "openMain" });
    c.say({ type: "action", action: "openSettings" });
    c.say({ type: "action", action: "openAccess" });
    c.say({ type: "action", action: "dismissAccess" });
    c.say({ type: "action", action: "drop", spaceId: "local:aurora", paths: ["/Applications/Notes.app"] });
    c.say({ type: "action", action: "openPermissionSettings", pane: "accessibility" });
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    expect(calls).toEqual([
      ["openSpace", "local:aurora"],
      ["openMain"],
      ["openSettings"],
      ["openAccess"],
      ["dismissAccess"],
      ["drop", "local:aurora", ["/Applications/Notes.app"]],
    ]);
    expect(requested).toEqual([1]);
    expect(opened[0]).toMatch(/Privacy_Accessibility/);
    // The permission is missing: the open panel shows the core's line.
    expect(fake.events.some((e) => e.tag === "DragPermission" && e.inner?.granted === false)).toBe(true);
    expect((c.received.at(-1)?.view as { permission?: unknown }).permission).toBeDefined();
  });

  it("sends thumbnails while open and gives a restarted helper everything again", async () => {
    const { children, clock } = setup();
    const c = children[0]!;
    c.say({ type: "stage", open: true });
    await vi.waitFor(() => expect(c.received.some((m) => m.type === "thumbnail")).toBe(true));
    expect(c.received.find((m) => m.type === "thumbnail")).toEqual({ type: "thumbnail", id: "local:aurora", image: "iVBORw==" });
    c.exitCode = 1;
    c.emit("exit", 1, null);
    clock.advance(500);
    const again = children[1]!;
    await vi.waitFor(() => expect(again.received.map((m) => m.type)).toEqual(["hello", "state", "thumbnail"]));
  });

  // The recheck on a Mac without a notch: the setting taken over from the
  // SwiftUI app hides the notch at launch, the helper reports a screen with
  // no notch, and turning the setting on shows it with a layout for that
  // screen (the panel is a layer-27 window, which `list_windows` leaves out).
  it("shows a notch hidden from launch, laid out for a screen without a notch, when the setting turns it on", async () => {
    const lines: string[] = [];
    const { notch, children } = setup((l) => lines.push(l));
    const c = children[0]!;
    notch.setShown(false);
    const wide = { frame: { x: 0, y: 0, width: 3440, height: 1440 }, visibleFrame: { x: 0, y: 90, width: 3440, height: 1320 }, safeAreaTop: 0 };
    c.say({ type: "screens", notch: wide, primary: wide });
    await vi.waitFor(() => expect(c.received.at(-1)?.layout).toBeDefined());
    expect(c.received.at(-1)?.shown).toBe(false);
    notch.setShown(true);
    await vi.waitFor(() => expect(c.received.at(-1)?.shown).toBe(true));
    expect(c.received.at(-1)?.layout).toEqual(c.received.at(-2)?.layout);
    // The trace has both ways, images left out.
    expect(lines[0]).toMatch(/^started \/helper/);
    expect(lines.some((l) => l.startsWith('-> {"type":"hello"'))).toBe(true);
    expect(lines.some((l) => l.startsWith('<- {"type":"screens"'))).toBe(true);
    expect(lines.some((l) => l.startsWith("-> ") && l.includes('"shown":true'))).toBe(true);
  });

  it("follows the notch setting", async () => {
    const { notch, children } = setup();
    const c = children[0]!;
    notch.setShown(false);
    await vi.waitFor(() => expect(c.received.at(-1)?.shown).toBe(false));
    notch.setShown(true);
    await vi.waitFor(() => expect(c.received.at(-1)?.shown).toBe(true));
  });

  it("stops the helper (quit, stdin closed) and every timer", async () => {
    const { notch, children, clock } = setup();
    const c = children[0]!;
    const ended = new Promise((r) => c.stdin.on("finish", r));
    notch.stop();
    await ended;
    expect(c.received.at(-1)).toEqual({ type: "quit" });
    c.exitCode = 0;
    c.emit("exit", 0, null);
    clock.advance(120_000);
    expect(children).toHaveLength(1);
  });
});

describe("window drags onto the app's own Space tiles (fake helper)", () => {
  it("asks the helper which window is under the dragged one, and highlights the tile only when it is the app's", async () => {
    const highlights: (string | null)[] = [];
    let listener: { onEvent(e: WindowDragEvent): void } | undefined;
    const windowTargets: WindowTargets = {
      at: (x) => (x < 300 ? { spaceId: "local:aurora", window: 900 } : null),
      highlight: (id) => void highlights.push(id),
    };
    const { children, fake } = setup(undefined, { windowTargets, onListener: (l) => (listener = l) });
    const c = children[0]!;
    await vi.waitFor(() => expect(listener).toBeDefined());
    fake.script(() => ({ phase: 1, kind: 1, overlay: [] }));
    listener!.onEvent({ phase: "start", x: 700, y: 500, window: { windowId: 7, appName: "Notes" } });
    listener!.onEvent({ phase: "move", x: 150, y: 120 });
    await vi.waitFor(() => expect(c.received.at(-1)).toEqual({ type: "windowUnder", id: 1, x: 150, y: 120, below: 7 }));
    c.say({ type: "windowUnder", id: 1, window: 900 });
    await vi.waitFor(() => expect(highlights).toEqual(["local:aurora"]));
    // Another window over the tile there: no highlight.
    listener!.onEvent({ phase: "move", x: 160, y: 120 });
    await vi.waitFor(() => expect(c.received.at(-1)).toMatchObject({ type: "windowUnder", id: 2 }));
    c.say({ type: "windowUnder", id: 2, window: 1234 });
    await vi.waitFor(() => expect(highlights).toEqual(["local:aurora", null]));
    // A helper that never answers: none, after the wait.
    listener!.onEvent({ phase: "move", x: 170, y: 120 });
    await new Promise((r) => setTimeout(r, 80));
    expect(highlights).toEqual(["local:aurora", null]);
  });
});

describe("window drags onto the notch", () => {
    it("follows window drags with the SDK's teleport handle once there is one", async () => {
      let handle: { windowDragSupported(): boolean; windowDragPermitted(): boolean; requestWindowDragPermission(): boolean; startWindowDrag: ReturnType<typeof vi.fn>; captureWindowThumbnail(): ArrayBuffer } | null = null;
      const t = notchTeleport(() => handle as never);
      expect(t.windowDragSupported()).toBe(false);
      expect(t.windowDragPermitted()).toBe(false);
      await expect(t.startWindowDrag({ onEvent() {} })).rejects.toThrow(/not running/);
      const monitor = { stop: vi.fn() };
      handle = {
        windowDragSupported: () => true,
        windowDragPermitted: () => true,
        requestWindowDragPermission: () => true,
        startWindowDrag: vi.fn(() => monitor),
        captureWindowThumbnail: () => new ArrayBuffer(3),
      };
      expect(t.windowDragSupported()).toBe(true);
      const listener = { onEvent() {} };
      const started = t.startWindowDrag(listener);
      // Not inside the caller's own call: the event tap may take a while to start.
      expect(handle.startWindowDrag).not.toHaveBeenCalled();
      expect(await started).toBe(monitor);
      expect(handle.startWindowDrag).toHaveBeenCalledWith(listener);
      expect((t.captureWindowThumbnail(7, 320) as ArrayBuffer).byteLength).toBe(3);
    });
});

// CUA-1618: window-drag tracking needs both the SDK (the launch's end) and
// the notch (its helper connecting), in either order, and follows the
// "Spaces tab in the notch" setting live.
describe("window-drag tracking, whichever comes first", () => {
  function lifecycle(o: { helperUp: boolean }) {
    const clock = manualClock();
    const children: FakeChild[] = [];
    let helperUp = o.helperUp;
    const monitors: { stop: ReturnType<typeof vi.fn> }[] = [];
    // The SDK's teleport handle: none until the launch hands it over.
    let sdk: unknown = null;
    const sdkIn = () => {
      sdk = {
        windowDragSupported: () => true,
        windowDragPermitted: () => true,
        requestWindowDragPermission: () => true,
        startWindowDrag: () => {
          const m = { stop: vi.fn() };
          monitors.push(m);
          return m;
        },
        captureWindowThumbnail: () => null,
      };
    };
    const listeners = new Set<() => void>();
    const startup = {
      isReady: false,
      subscribe(l: () => void) {
        listeners.add(l);
        return () => void listeners.delete(l);
      },
      ready() {
        this.isReady = true;
        for (const l of [...listeners]) l();
      },
    };
    const notch = startNotch(
      {
        core: fakeCore().core,
        actions: { openSpace() {}, openMain() {}, openSettings() {}, openAccess() {}, dismissAccess() {}, teleport() {}, drop() {} },
        teleport: notchTeleport(() => sdk as never),
      },
      {
        path: "/helper",
        clock,
        log: () => {},
        process: {
          // The helper still connecting: its first start fails and it restarts after the backoff.
          spawn: () => {
            if (!helperUp) {
              helperUp = true;
              throw new Error("not yet");
            }
            const c = new FakeChild();
            children.push(c);
            return c as unknown as ChildProcess;
          },
          after: clock.after,
          now: clock.now,
        },
      },
    );
    /** Lets the monitor's deferred start (`notchTeleport`) run. */
    const settle = () => new Promise((r) => setTimeout(r, 5));
    return { notch, clock, children, monitors, sdkIn, startup, settle };
  }

  it("starts when the launch finished before the notch was wired to it (the startup race)", async () => {
    const { notch, monitors, sdkIn, startup, settle, clock, children } = lifecycle({ helperUp: false });
    await settle();
    expect(monitors).toHaveLength(0);
    // The launch ends while the notch is still connecting: no change is seen after the wiring.
    sdkIn();
    startup.isReady = true;
    followDragReadiness(notch, startup);
    await settle();
    expect(monitors).toHaveLength(1);
    // The helper connects later: still one monitor.
    clock.advance(500);
    expect(children).toHaveLength(1);
    await settle();
    expect(monitors).toHaveLength(1);
    notch.stop();
  });

  it("starts once a delayed helper connects, the SDK having come in meanwhile", async () => {
    const { notch, monitors, sdkIn, settle, clock, children } = lifecycle({ helperUp: false });
    await settle();
    expect(children).toHaveLength(0);
    sdkIn();
    await settle();
    expect(monitors).toHaveLength(0);
    clock.advance(500);
    expect(children).toHaveLength(1);
    await vi.waitFor(() => expect(monitors).toHaveLength(1));
    notch.stop();
  });

  it("starts once the launch finishes after the notch connected", async () => {
    const { notch, monitors, sdkIn, startup, settle } = lifecycle({ helperUp: true });
    followDragReadiness(notch, startup);
    await settle();
    expect(monitors).toHaveLength(0);
    sdkIn();
    startup.ready();
    await vi.waitFor(() => expect(monitors).toHaveLength(1));
    notch.stop();
  });

  it("stops when the setting hides the notch and starts again when it shows it, without a relaunch", async () => {
    const { notch, monitors, sdkIn, startup, settle } = lifecycle({ helperUp: true });
    notch.setShown(false);
    sdkIn();
    startup.ready();
    followDragReadiness(notch, startup);
    await settle();
    expect(monitors).toHaveLength(0);
    // Turned on while the app runs.
    notch.setShown(true);
    await vi.waitFor(() => expect(monitors).toHaveLength(1));
    notch.setShown(false);
    expect(monitors[0]!.stop).toHaveBeenCalledTimes(1);
    notch.setShown(true);
    await vi.waitFor(() => expect(monitors).toHaveLength(2));
    expect(monitors[1]!.stop).not.toHaveBeenCalled();
    notch.stop();
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import { createRequire } from "node:module";
import * as path from "node:path";
import { PassThrough } from "node:stream";
import { describe, expect, it, vi } from "vitest";
import { helperPath, notchSupported, startNotch, type NotchActions } from "../src/notch";
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
    expect(helperPath({ packaged: true, resourcesPath: resources, appPath: `${resources}/app.asar` })).toBe(
      "/Applications/Cua Spaces.app/Contents/Helpers/Cua Spaces Notch.app/Contents/MacOS/Cua Spaces Notch",
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

function setup() {
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
        windowDragPermitted: () => false,
        requestWindowDragPermission: () => (requested.push(1), true),
        startWindowDrag: () => ({ stop() {} }),
        captureWindowThumbnail: () => null,
      },
    },
    {
      path: "/helper",
      clock,
      openURL: (u) => opened.push(u),
      log: () => {},
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

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CuaDesktopBridge } from "@/bridge/detect";
import type { PresenceJoinInfo } from "@/bridge/ops/presence";

import { attachPresence, resetPresenceSessions } from "./presence";
import { WebCodecsSlots, type PresenceFactory } from "./webcodecs-slots";

const ME = { participantId: "p1", principalId: "user:ada", displayName: "Ada", color: "#e6194b", kind: "human" };
const BOB = { participantId: "p2", principalId: "user:bob", displayName: "Bob", color: "#4363d8", kind: "human" };

function cursor(x: number, y: number, windowId?: string) {
  return { displayId: "", ...(windowId ? { windowId } : {}), x, y, visible: true, pressed: false, shape: "arrow", shapeSource: "unspecified", atMs: 0, receivedMs: 0 };
}

/** A shell that joins `spaceId` as Ada with Bob already there, and pushes events to the page. */
function fakeShell(join: Partial<PresenceJoinInfo> = {}) {
  const calls: { method: string; args: Record<string, unknown> }[] = [];
  const listeners = new Set<(payload: unknown) => void>();
  const desktop: CuaDesktopBridge = {
    invoke: async (_channel, request) => {
      const r = request as { id: string; method: string; args: Record<string, unknown> };
      calls.push({ method: r.method, args: r.args });
      if (r.method === "presence.join") {
        const info: PresenceJoinInfo = { handle: "h1", me: ME, members: [{ participant: BOB, cursor: cursor(0.5, 0.5) }], delayMs: 0, datagrams: false, ...join };
        return { id: r.id, ok: true, result: info };
      }
      return { id: r.id, ok: true, result: null };
    },
    on: (_channel, listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  const push = (handle: string, event: unknown) => listeners.forEach((l) => l({ event: "presence.event", payload: { handle, event } }));
  return { desktop, calls, push, listeners };
}

function surface() {
  const host = document.createElement("div");
  const canvas = document.createElement("canvas");
  host.append(canvas);
  document.body.append(host);
  return { host, canvas };
}

const flush = () => new Promise((r) => setTimeout(r, 0));
const nextFrame = () => new Promise((r) => requestAnimationFrame(() => r(null)));

beforeEach(() => resetPresenceSessions());
afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("presence over the page's video", () => {
  it("draws the other participants with their name and color, only while the stream is live", async () => {
    const shell = fakeShell();
    const { host, canvas } = surface();
    const p = attachPresence(shell.desktop, canvas, host, { spaceId: "local:a", interactive: true });
    await flush();
    await nextFrame();
    const layer = host.querySelector<HTMLElement>(".presence-layer");
    expect(layer?.style.display).toBe("none");
    const bob = host.querySelector<HTMLElement>('.presence-cursor[data-participant="p2"]');
    expect(bob?.textContent).toBe("Bob");
    expect(bob?.innerHTML).toContain("#4363d8");
    // You are never drawn as someone else.
    expect(host.querySelector('.presence-cursor[data-participant="p1"]:not([data-me])')).toBeNull();

    p.setLive(true);
    expect(layer?.style.display).toBe("");
    p.setLive(false);
    expect(layer?.style.display).toBe("none");
    p.detach();
    expect(host.querySelector(".presence-layer")).toBeNull();
  });

  it("folds the host's events for its own session only, and draws a window's cursors on that window only", async () => {
    const shell = fakeShell({ members: [] });
    const desk = surface();
    const win = surface();
    const a = attachPresence(shell.desktop, desk.canvas, desk.host, { spaceId: "local:a", interactive: true });
    const b = attachPresence(shell.desktop, win.canvas, win.host, { spaceId: "local:a", windowId: "w7", interactive: true });
    await flush();
    // One participant per Space per page, however many slots show it.
    expect(shell.calls.filter((c) => c.method === "presence.join")).toHaveLength(1);
    shell.push("h1", { kind: "joined", participant: BOB });
    shell.push("h1", { kind: "cursor_moved", participantId: "p2", cursor: cursor(0.25, 0.25, "w7") });
    shell.push("other", { kind: "joined", participant: { ...BOB, participantId: "p3", displayName: "Eve" } });
    await nextFrame();
    expect(win.host.querySelector('.presence-cursor[data-participant="p2"]')).not.toBeNull();
    expect(desk.host.querySelector('.presence-cursor[data-participant="p2"]')).toBeNull();
    expect(win.host.querySelector('.presence-cursor[data-participant="p3"]')).toBeNull();

    a.detach();
    await flush();
    expect(shell.calls.some((c) => c.method === "presence.leave")).toBe(false);
    b.detach();
    await flush();
    expect(shell.calls.filter((c) => c.method === "presence.leave")).toEqual([{ method: "presence.leave", args: { handle: "h1" } }]);
    expect(shell.listeners.size).toBe(0);
  });

  it("publishes your pointer over the picture, and a hide when it leaves", async () => {
    const shell = fakeShell({ members: [] });
    const { host, canvas } = surface();
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({ left: 0, top: 0, width: 200, height: 100, right: 200, bottom: 100, x: 0, y: 0, toJSON: () => ({}) });
    const p = attachPresence(shell.desktop, canvas, host, { spaceId: "local:a", interactive: true });
    await flush();
    p.setLive(true);
    canvas.dispatchEvent(Object.assign(new Event("pointermove"), { clientX: 50, clientY: 25 }));
    canvas.dispatchEvent(new Event("pointerleave"));
    await flush();
    const published = shell.calls.filter((c) => c.method === "presence.publish").map((c) => c.args);
    expect(published).toEqual([
      { handle: "h1", x: 0.25, y: 0.25, visible: true },
      { handle: "h1", x: 0, y: 0, visible: false },
    ]);
    p.detach();
  });

  it("leaves the video alone when the Space has no presence", async () => {
    const shell = fakeShell();
    shell.desktop.invoke = async (_c, request) => ({ id: (request as { id: string }).id, ok: false, error: { code: "unsupported", message: "no presence" } });
    const { host, canvas } = surface();
    const p = attachPresence(shell.desktop, canvas, host, { spaceId: "local:a", interactive: true });
    await flush();
    p.setLive(true);
    expect(host.querySelector(".presence-layer")).toBeNull();
    p.detach();
  });
});

describe("the WebCodecs slots", () => {
  it("draw presence on viewer slots only, live with the stream, and take it down with the slot", async () => {
    const attached: { options: Parameters<PresenceFactory>[3]; live: boolean[]; detached: boolean }[] = [];
    const factory: PresenceFactory = (_d, _c, _h, options) => {
      const rec = { options, live: [] as boolean[], detached: false };
      attached.push(rec);
      return { setLive: (l) => rec.live.push(l), detach: () => (rec.detached = true) };
    };
    const desktop: CuaDesktopBridge = {
      invoke: async (_c, request) => ({ id: (request as { id: string }).id, ok: true, result: { wsUrl: "ws://127.0.0.1:1/x", expiresAt: null } }),
      platform: "linux",
    };
    let onFrame: ((w: number, h: number) => void) | undefined;
    const slots = new WebCodecsSlots(
      desktop,
      document,
      (o) => {
        onFrame = o.onFrame;
        return { start() {}, stop() {} };
      },
      null,
      factory,
    );
    const el = document.createElement("div");
    document.body.append(el);
    const stopTile = slots.register(el, { spaceId: "local:a", tier: "tile", interactive: false, radius: 8 }, () => {});
    expect(attached).toHaveLength(0);
    stopTile();
    const stop = slots.register(el, { spaceId: "local:a", tier: "full", interactive: true, radius: 0, windowId: "w7", epoch: 2 }, () => {});
    expect(attached.map((a) => a.options)).toEqual([{ spaceId: "local:a", windowId: "w7", interactive: true }]);
    await flush();
    onFrame?.(1280, 720);
    expect(attached[0]?.live.at(-1)).toBe(true);
    stop();
    expect(attached[0]?.detached).toBe(true);
  });
});

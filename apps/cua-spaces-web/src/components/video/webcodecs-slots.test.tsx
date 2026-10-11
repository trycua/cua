// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore } from "@/bridge/__tests__/testCore";
import { BridgeProvider } from "@/bridge";
import type { MachineAccessNotice } from "@/bridge/contracts/devices";
import type { CuaDesktopBridge } from "@/bridge/detect";
import { StreamSurface } from "@/components/space-detail/stream-surface";
import { ToastProvider } from "@/components/ui/toast";
import type { SlotStatus } from "@/lib/video-slots";

import { resetVideoSlotLoader } from "./use-video-slot";
import { NO_VIDEO_HERE, webCodecsHost } from "./webcodecs-host";
import { VideoStats } from "./video-stats";
import { needsActivation, resetWebCodecsSlots, sessionConfig, streamTarget, WebCodecsSlots, type SessionFactory } from "./webcodecs-slots";

// The production MediaSession, replaced: jsdom has no WebSocket media or VideoDecoder.
const sessions: { options: Parameters<SessionFactory>[0]; started: number; stopped: number }[] = [];
vi.mock("../../../../../libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession", () => ({
  MediaSession: class {
    rec: (typeof sessions)[number];
    constructor(options: Parameters<SessionFactory>[0]) {
      this.rec = { options, started: 0, stopped: 0 };
      sessions.push(this.rec);
    }
    start() {
      this.rec.started += 1;
    }
    stop() {
      this.rec.stopped += 1;
    }
  },
}));

type Reply = { ok: true; result: { wsUrl: string; expiresAt: string | null } } | { ok: false; error: { message: string; code?: string } };

function fakeDesktop(reply: (args: { spaceId: string; tier: string }) => Reply = ({ spaceId, tier }) => ({ ok: true, result: { wsUrl: `ws://127.0.0.1:5123/v1/bridge/media?ticket=${spaceId}-${tier}`, expiresAt: null } })) {
  type Request = { id: string; method: string; args: { spaceId: string; tier: string } };
  const calls: { channel: string; method: string; args: { spaceId: string; tier: string } }[] = [];
  const desktop: CuaDesktopBridge = {
    invoke: async (channel: string, request?: unknown) => {
      const r = request as Request;
      calls.push({ channel, method: r.method, args: r.args });
      return { id: r.id, ...reply(r.args) };
    },
    platform: "linux",
  };
  return { desktop, calls };
}

let hidden = false;
const registered: (() => void)[] = [];
beforeEach(() => {
  sessions.length = 0;
  hidden = false;
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => (hidden ? "hidden" : "visible") });
  resetVideoSlotLoader();
  resetWebCodecsSlots();
});
afterEach(() => {
  for (const stop of registered.splice(0)) stop();
  cleanup();
  delete (window as { cuaDesktop?: unknown }).cuaDesktop;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

const flush = () => new Promise((r) => setTimeout(r, 0));
const tile = { spaceId: "local:a", tier: "tile" as const, interactive: false, radius: 8 };
const viewer = { spaceId: "local:a", tier: "full" as const, interactive: true, radius: 12, inset: 1 };

function slotIn(desktop: CuaDesktopBridge, options: Parameters<WebCodecsSlots["register"]>[1] = tile, stats?: VideoStats) {
  const slots = new WebCodecsSlots(desktop, document, undefined, stats);
  const el = document.createElement("div");
  document.body.append(el);
  const seen: SlotStatus[] = [];
  const stop = slots.register(el, options, (s) => seen.push(s));
  registered.push(stop);
  return { slots, el, seen, stop, last: () => seen.at(-1) };
}

describe("webCodecsHost", () => {
  const desktop = { invoke: async () => null };
  it("is the Electron shell only when Chromium can decode and no host draws native video", () => {
    expect(webCodecsHost({})).toBeNull();
    expect(webCodecsHost({ cuaDesktop: desktop })).toBeNull();
    expect(webCodecsHost({ cuaDesktop: desktop, VideoDecoder: class {} })).toBe(desktop);
    expect(webCodecsHost({ cuaDesktop: desktop, VideoDecoder: class {}, webkit: { messageHandlers: { cuaVideo: { postMessage() {} } } } })).toBeNull();
    expect(webCodecsHost({ cuaDesktop: desktop, VideoDecoder: class {}, location: { search: "?bridge=demo" } })).toBeNull();
  });
});

describe("WebCodecsSlots", () => {
  it("asks the shell for a tile ticket, attaches a view-only session to a hidden canvas, and shows it from the first frame", async () => {
    const { desktop, calls } = fakeDesktop();
    const { el, seen, last, stop } = slotIn(desktop);
    // It starts as connecting, which the hook already assumes.
    expect(seen).toEqual([]);
    await flush();
    expect(calls).toEqual([{ channel: "cua:bridge", method: "spaces.openStream", args: { spaceId: "local:a", tier: "tile" } }]);
    const canvas = el.querySelector<HTMLCanvasElement>('canvas[data-webcodecs="tile"]')!;
    expect(canvas.style.opacity).toBe("0");
    const s = sessions[0]!;
    expect(s.started).toBe(1);
    expect(s.options).toMatchObject({ canvas, interactive: false, audio: false, ticket: { wsUrl: "ws://127.0.0.1:5123/v1/bridge/media?ticket=local:a-tile" } });

    act(() => s.options.onFrame?.(960, 600));
    expect(last()).toEqual({ phase: "live", focused: false });
    expect(canvas.style.opacity).toBe("1");

    // A new ticket when the old one expires or the socket closes with 4401.
    await s.options.reopen?.();
    expect(calls).toHaveLength(2);

    stop();
    expect(s.stopped).toBe(1);
    expect(el.querySelector("canvas")).toBeNull();
  });

  it("says there is no video here when the shell can't stream, and opens no session", async () => {
    const { desktop } = fakeDesktop(() => ({ ok: false, error: { message: "Live video needs cua on this computer.", code: "unsupported" } }));
    const { last } = slotIn(desktop);
    await flush();
    expect(last()).toEqual({ phase: "failed", reason: NO_VIDEO_HERE, opening: true, focused: false });
    expect(sessions).toHaveLength(0);
  });

  it("fails with the daemon's reason, and when the session ends for good", async () => {
    const bad = slotIn(fakeDesktop(() => ({ ok: false, error: { message: "no sandbox named local:a" } })).desktop);
    await flush();
    expect(bad.last()).toMatchObject({ phase: "failed", reason: "no sandbox named local:a", opening: true });

    const good = slotIn(fakeDesktop().desktop);
    await flush();
    const s = sessions[0]!;
    act(() => s.options.onGone?.("the window is gone"));
    expect(good.last()).toMatchObject({ phase: "failed", reason: "the window is gone" });
    // It was open: only its cover says so.
    expect(good.last()?.opening).toBeUndefined();
    expect(s.stopped).toBe(1);
  });

  it("closes the session while the page is hidden and reopens with a fresh ticket", async () => {
    const { desktop, calls } = fakeDesktop();
    const { last } = slotIn(desktop);
    await flush();
    act(() => sessions[0]!.options.onFrame?.(960, 600));
    hidden = true;
    document.dispatchEvent(new Event("visibilitychange"));
    expect(sessions[0]!.stopped).toBe(1);
    expect(last()?.phase).toBe("connecting");
    hidden = false;
    document.dispatchEvent(new Event("visibilitychange"));
    await flush();
    expect(calls).toHaveLength(2);
    expect(sessions).toHaveLength(2);
    expect(sessions[1]!.started).toBe(1);
  });

  it("drops a ticket that arrives after the slot went away", async () => {
    let answer: (r: Reply) => void = () => {};
    const desktop: CuaDesktopBridge = { invoke: () => new Promise((r) => (answer = r)) };
    const { stop } = slotIn(desktop);
    stop();
    answer({ ok: true, result: { wsUrl: "ws://127.0.0.1:1/m", expiresAt: null } });
    await flush();
    expect(sessions).toHaveLength(0);
  });

  it("gives the viewer the keyboard on focus and takes it back on Ctrl+Shift+F12 off the Mac, never on Ctrl+Esc", async () => {
    const { slots, el, last } = slotIn(fakeDesktop().desktop, viewer);
    await flush();
    expect(sessions[0]!.options.interactive).toBe(true);
    const canvas = el.querySelector("canvas")!;
    expect(canvas.tabIndex).toBe(0);
    expect(canvas.style.inset).toBe("1px");
    expect(canvas.hasAttribute("data-space-keyboard")).toBe(true);

    act(() => slots.focus(slots.idOf(el)));
    expect(document.activeElement).toBe(canvas);
    expect(last()?.focused).toBe(true);

    // Ctrl+Esc is the Start menu on Windows: it stays with the Space.
    const ctrlEsc = new KeyboardEvent("keydown", { key: "Escape", ctrlKey: true, bubbles: true, cancelable: true });
    act(() => void canvas.dispatchEvent(ctrlEsc));
    expect(ctrlEsc.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(canvas);

    const release = new KeyboardEvent("keydown", { key: "F12", ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true });
    act(() => void canvas.dispatchEvent(release));
    expect(release.defaultPrevented).toBe(true);
    expect(document.activeElement).not.toBe(canvas);
    expect(last()?.focused).toBe(false);

    act(() => slots.focus(slots.idOf(el)));
    act(() => slots.focus(null));
    expect(last()?.focused).toBe(false);
  });

  // The Swift app's KeyCapture (#4609): every Command chord, ⌘Esc too, goes to the Space;
  // Control+Option pressed and released alone gives the keyboard back.
  it("on a Mac sends Command chords to the Space and gives the keyboard back on Control+Option alone", async () => {
    const { slots, el, last } = slotIn({ ...fakeDesktop().desktop, platform: "darwin" }, viewer);
    await flush();
    const canvas = el.querySelector("canvas")!;
    const key = (type: "keydown" | "keyup", k: string, init: KeyboardEventInit = {}) => {
      const e = new KeyboardEvent(type, { key: k, bubbles: true, cancelable: true, ...init });
      act(() => void canvas.dispatchEvent(e));
      return e;
    };
    const page: string[] = [];
    const onPage = (e: KeyboardEvent) => page.push(e.key);
    window.addEventListener("keydown", onPage);
    try {
      act(() => slots.focus(slots.idOf(el)));
      key("keydown", "Escape", { metaKey: true });
      key("keydown", "k", { metaKey: true });
      expect(document.activeElement, "⌘Esc is the Space's").toBe(canvas);
      expect(page, "the page's shortcuts don't see the Space's keys").toEqual([]);

      // ⌃⌥T is a chord for the Space, not the release.
      key("keydown", "Control", { ctrlKey: true });
      key("keydown", "Alt", { ctrlKey: true, altKey: true });
      key("keydown", "t", { ctrlKey: true, altKey: true });
      key("keyup", "Alt", { ctrlKey: true });
      key("keyup", "Control");
      expect(document.activeElement).toBe(canvas);

      key("keydown", "Control", { ctrlKey: true });
      key("keydown", "Alt", { ctrlKey: true, altKey: true });
      key("keyup", "Control", { altKey: true });
      key("keyup", "Alt");
      expect(document.activeElement).not.toBe(canvas);
      expect(last()?.focused).toBe(false);

      // Back on the page, its shortcuts see keys again.
      act(() => void document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true })));
      expect(page).toEqual(["k"]);
    } finally {
      window.removeEventListener("keydown", onPage);
    }
  });
});

describe("decoding and input like the Swift app's video tiers", () => {
  it("asks every slot for the hardware decoder; the viewer sends ⌘ as Control to a Linux or Windows Space from a Mac", () => {
    expect(sessionConfig({ interactive: false, os: "linux" }, "darwin")).toEqual({ hardwareAcceleration: "prefer-hardware", metaAsControl: false, wholeCommandChords: false, scrollNeedsFocus: false });
    expect(sessionConfig({ interactive: true, os: "linux" }, "darwin")).toEqual({ hardwareAcceleration: "prefer-hardware", metaAsControl: true, wholeCommandChords: true, scrollNeedsFocus: true });
    // ⌘ chords go whole from a Mac only (Chromium drops their key-up there).
    expect(sessionConfig({ interactive: true, os: "macos" }, "darwin").wholeCommandChords).toBe(true);
    expect(sessionConfig({ interactive: true, os: "linux" }, "linux").wholeCommandChords).toBe(false);
    expect(sessionConfig({ interactive: true, os: "windows" }, "darwin").metaAsControl).toBe(true);
    expect(sessionConfig({ interactive: true, os: "macos" }, "darwin").metaAsControl).toBe(false);
    expect(sessionConfig({ interactive: true, os: "linux" }, "win32").metaAsControl).toBe(false);
    expect(sessionConfig({ interactive: true }, "darwin").metaAsControl).toBe(false);
  });

  it("passes the config to the viewer's session", async () => {
    slotIn({ ...fakeDesktop().desktop, platform: "darwin" }, { ...viewer, os: "linux" });
    await flush();
    expect(sessions[0]!.options).toMatchObject({ hardwareAcceleration: "prefer-hardware", metaAsControl: true, wholeCommandChords: true, scrollNeedsFocus: true });
  });

  it("asks for one window with its epoch, in the background, and activation only when asked for", () => {
    expect(streamTarget(tile)).toEqual({ spaceId: "local:a", tier: "tile" });
    expect(streamTarget({ ...viewer, windowId: "w-1", epoch: 3 })).toEqual({ spaceId: "local:a", tier: "full", windowId: "w-1", epoch: 3 });
    expect(streamTarget({ ...viewer, windowId: "w-1", epoch: 3 }, true)).toEqual({ spaceId: "local:a", tier: "full", windowId: "w-1", epoch: 3, activate: true });
    expect(streamTarget(viewer, true)).toEqual({ spaceId: "local:a", tier: "full" });
  });

  it("opens a window again with activation, once, when the Space says its input needs it", async () => {
    const { desktop, calls } = fakeDesktop();
    slotIn(desktop, { ...viewer, windowId: "w-1", epoch: 3 });
    await flush();
    act(() => sessions[0]!.options.onInputAck?.({ delivered: true }));
    expect(calls).toHaveLength(1);
    act(() => sessions[0]!.options.onInputAck?.({ delivered: false, error: { code: "would_require_activation" } }));
    await flush();
    expect(sessions[0]!.stopped).toBe(1);
    expect(calls.map((c) => c.args)).toEqual([
      { spaceId: "local:a", tier: "full", windowId: "w-1", epoch: 3 },
      { spaceId: "local:a", tier: "full", windowId: "w-1", epoch: 3, activate: true },
    ]);
    act(() => sessions[1]!.options.onInputAck?.({ delivered: false, error: { code: "would_require_activation" } }));
    await flush();
    expect(calls).toHaveLength(2);
    expect(needsActivation({ delivered: false, error: { code: "permission_denied" } })).toBe(false);
  });

  it("counts frames for the video bench: drawn, painted, dropped and arrival to paint", async () => {
    const paints: ((t: number) => void)[] = [];
    const stats = new VideoStats({ now: () => 0, nextPaint: (cb) => paints.push(cb), cancel: () => {} });
    const { desktop } = fakeDesktop();
    const { slots, el } = slotIn(desktop, tile, stats);
    await flush();
    const s = sessions[0]!;
    // Two frames drawn before one paint: one reaches the screen.
    act(() => {
      s.options.onFrameTiming?.({ receivedAt: 100, drawnAt: 104 });
      s.options.onFrameTiming?.({ receivedAt: 110, drawnAt: 113 });
    });
    paints.shift()!(120);
    expect(paints).toHaveLength(0);
    act(() => s.options.onFrameTiming?.({ receivedAt: 200, drawnAt: 205 }));
    paints.shift()!(216);
    stats.counts(slots.idOf(el)!, { received: 4, decoded: 3, width: 960, height: 600 });
    const [snap] = stats.snapshot();
    expect(snap).toMatchObject({ spaceId: "local:a", tier: "tile", received: 4, decoded: 3, presented: 3, painted: 2, dropped: 2, size: "960x600", failure: null });
    expect(snap!.latencyMs).toEqual([10, 16]);
    expect(snap!.decodeMs).toEqual([4, 3, 5]);
  });
});

describe("<StreamSurface> in the Electron shell", () => {
  const mount = () =>
    render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={noCore}>
        <StreamSurface spaceId="local:a" os="linux" canStream previewText="Not reachable" />
      </BridgeProvider>,
    );
  const electron = (desktop: CuaDesktopBridge) => {
    (window as { cuaDesktop?: unknown }).cuaDesktop = desktop;
    vi.stubGlobal("VideoDecoder", class {});
  };

  it("draws the Space's desktop in the page and goes live on the first frame", async () => {
    const { desktop, calls } = fakeDesktop();
    electron(desktop);
    const { container, getByText } = mount();
    expect(getByText("Connecting…")).toBeTruthy();
    await waitFor(() => expect(sessions).toHaveLength(1));
    expect(calls[0]?.args).toEqual({ spaceId: "local:a", tier: "full" });
    act(() => sessions[0]!.options.onFrame?.(1280, 800));
    await waitFor(() => expect(container.querySelector('[data-stream-state="live"]')).toBeTruthy());
    expect(container.querySelector('[data-stream-surface="local:a"] canvas[data-webcodecs="full"]')).toBeTruthy();
    expect(getByText("Click the desktop to control it.")).toBeTruthy();

    // A click gives it the keyboard; the line says how to give it back, in this platform's keys.
    vi.spyOn(navigator, "platform", "get").mockReturnValue("Linux x86_64");
    act(() => container.querySelector<HTMLCanvasElement>("canvas")!.focus());
    await waitFor(() => expect(container.querySelector('[data-video-focus="space"]')?.textContent).toContain("Ctrl+Shift+F12"));
  });

  // The SwiftUI app's ProviderFailureTests: the stream fails to open twice, then opens.
  describe.each([false, true])("when opening fails (auto-connect %s)", (autoConnect) => {
    it("offers Try again, says why once per attempt, never asks again by itself, then attempts media", async () => {
      let replies = 0;
      const { desktop, calls } = fakeDesktop(({ spaceId, tier }) =>
        ++replies <= 2 ? { ok: false, error: { message: "scripted provider failure" } } : { ok: true, result: { wsUrl: `ws://127.0.0.1:5123/v1/bridge/media?ticket=${spaceId}-${tier}`, expiresAt: null } },
      );
      electron(desktop);
      const demo = createDemoAdapter({ latencyMs: 1 });
      const adapter = {
        ...demo,
        call: (async (op: string, args: never) => {
          const out = await demo.call(op as never, args);
          if (op !== "settings.get") return out;
          const snap = out as { values: object };
          return { ...snap, values: { ...snap.values, autoConnect } };
        }) as typeof demo.call,
      };
      const ui = () => (
        <BridgeProvider adapter={adapter} core={noCore}>
          <ToastProvider>
            <StreamSurface spaceId="local:a" os="linux" canStream previewText="Not reachable" />
          </ToastProvider>
        </BridgeProvider>
      );
      const view = render(ui());
      const toasts = () => view.queryAllByText("Could not open local:a").length;
      if (!autoConnect) {
        const connect = await view.findByRole("button", { name: "Connect" });
        await new Promise((r) => setTimeout(r, 20));
        expect(calls).toEqual([]);
        act(() => connect.click());
      }
      await waitFor(() => expect(view.container.querySelector('[data-stream-state="failed"]')).toBeTruthy());
      expect(calls).toHaveLength(1);
      await waitFor(() => expect(toasts()).toBe(1));
      expect(view.getByText("scripted provider failure")).toBeTruthy();
      // A poll (the page drawn again) does not try again.
      view.rerender(ui());
      await new Promise((r) => setTimeout(r, 20));
      expect(calls).toHaveLength(1);

      for (const attempt of [2, 3]) {
        act(() => view.getByRole("button", { name: "Try again" }).click());
        await waitFor(() => expect(calls).toHaveLength(attempt));
      }
      await waitFor(() => expect(sessions).toHaveLength(1));
      expect(sessions[0]!.started).toBe(1);
      expect(toasts()).toBe(2);
      view.rerender(ui());
      await new Promise((r) => setTimeout(r, 20));
      expect(sessions).toHaveLength(1);
      expect(calls).toHaveLength(3);
    });
  });

  it("says nothing more when a stream that was open fails: its cover offers Try again", async () => {
    const { desktop } = fakeDesktop();
    electron(desktop);
    const view = render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={noCore}>
        <ToastProvider>
          <StreamSurface spaceId="local:a" os="linux" canStream previewText="Not reachable" />
        </ToastProvider>
      </BridgeProvider>,
    );
    await waitFor(() => expect(sessions).toHaveLength(1));
    act(() => sessions[0]!.options.onGone?.("the stream ended"));
    await waitFor(() => expect(view.getByRole("button", { name: "Try again" })).toBeTruthy());
    expect(view.queryByText("Could not open local:a")).toBeNull();
  });

  // The SwiftUI app's DetailApprovalTests: the open detail follows this device's approval.
  const NOTICE: MachineAccessNotice = { kind: "needs-enrollment", status: "Not enrolled", text: "Enroll this Mac to connect to your other machines.", actionLabel: "Enroll This Mac…" };
  const withAccess = () => {
    const adapter = createDemoAdapter({ latencyMs: 1 });
    const ui = (access: MachineAccessNotice | null) => (
      <BridgeProvider adapter={adapter} core={noCore}>
        <StreamSurface spaceId="relay:studio" os="macos" canStream previewText="Not reachable" access={access} />
      </BridgeProvider>
    );
    const view = render(ui(NOTICE));
    return { view, set: (access: MachineAccessNotice | null) => view.rerender(ui(access)) };
  };

  it("opens the stream once this device is approved, in the same detail, and not again on repeated answers", async () => {
    const { desktop, calls } = fakeDesktop();
    electron(desktop);
    const { view, set } = withAccess();
    await new Promise((r) => setTimeout(r, 20));
    expect(calls).toEqual([]);
    expect(view.getByText(NOTICE.text)).toBeTruthy();

    set(null);
    await waitFor(() => expect(sessions).toHaveLength(1));
    expect(calls.map((c) => c.args)).toEqual([{ spaceId: "relay:studio", tier: "full" }]);
    set(null);
    set(null);
    await new Promise((r) => setTimeout(r, 20));
    expect(calls).toHaveLength(1);
    expect(sessions[0]!.stopped).toBe(0);
  });

  it("drops a ticket that comes after the approval was taken back, and asks again on the next approval", async () => {
    const pending: ((reply: Reply) => void)[] = [];
    const calls: string[] = [];
    electron({
      invoke: (_channel: string, request?: unknown) => {
        const r = request as { id: string; args: { spaceId: string } };
        calls.push(r.args.spaceId);
        return new Promise((resolve) => pending.push((reply) => resolve({ id: r.id, ...reply })));
      },
      platform: "linux",
    });
    const { view, set } = withAccess();
    set(null);
    await waitFor(() => expect(calls).toHaveLength(1));
    set(NOTICE);
    // The shell answers anyway (it does not hear the page went away): nothing of it shows.
    pending[0]!({ ok: false, error: { message: "cancelled provider result" } });
    await new Promise((r) => setTimeout(r, 20));
    expect(view.container.querySelector('[data-stream-state="failed"]')).toBeNull();
    expect(view.queryByText(/cancelled provider result/)).toBeNull();
    expect(view.getByText(NOTICE.text)).toBeTruthy();

    set(null);
    await waitFor(() => expect(calls).toHaveLength(2));
    pending[1]!({ ok: true, result: { wsUrl: "ws://127.0.0.1:5123/v1/bridge/media?ticket=t", expiresAt: null } });
    await waitFor(() => expect(sessions).toHaveLength(1));
  });

  it("keeps the plain placeholder when the shell has no video (no cua, sample data)", async () => {
    electron(fakeDesktop(() => ({ ok: false, error: { message: "Live video needs cua on this computer.", code: "unsupported" } })).desktop);
    const { container, findByText, queryByText } = mount();
    expect(await findByText("The desktop opens in its own window.")).toBeTruthy();
    expect(container.querySelector('[data-stream-state="placeholder"]')).toBeTruthy();
    expect(queryByText(/didn't start/)).toBeNull();
  });
});

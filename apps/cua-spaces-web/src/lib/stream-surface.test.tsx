// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore } from "@/bridge/__tests__/testCore";
import { BridgeProvider, type Space } from "@/bridge";
import { StreamSurface } from "@/components/space-detail/stream-surface";
import { ToastProvider } from "@/components/ui/toast";
import { canStreamTile } from "@/components/video/tile-video";
import { resetVideoSlotLoader } from "@/components/video/use-video-slot";
import { nativeVideoHandler, type StreamSurfaceMessage, type SurfaceState } from "@/lib/stream-surface";
import { intersect, isOccluded, resetVideoSlots, roundRect, sameSurface, visiblePart, VideoSlotReporter, type SlotStatus } from "@/lib/video-slots";

type Win = typeof window & { webkit?: unknown };

class FakeResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

/** Gives `el` a layout box (jsdom has none). */
function box(el: Element, r: { x: number; y: number; width: number; height: number }) {
  el.getBoundingClientRect = () => ({ ...r, top: r.y, left: r.x, right: r.x + r.width, bottom: r.y + r.height, toJSON: () => r }) as DOMRect;
}

function hostEvent(event: string, payload: unknown) {
  window.dispatchEvent(new CustomEvent("cua:event", { detail: { event, payload } }));
}

const surface = (over: Partial<SurfaceState> = {}): SurfaceState => ({
  surfaceId: "s",
  spaceId: "a",
  tier: "full",
  interactive: true,
  rect: { x: 1, y: 2, width: 3, height: 4 },
  clip: { x: 1, y: 2, width: 3, height: 4 },
  radius: 8,
  occluded: false,
  visible: true,
  ...over,
});

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  resetVideoSlots();
  resetVideoSlotLoader();
});

afterEach(() => {
  cleanup();
  delete (window as Win).webkit;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  resetVideoSlots();
  document.body.innerHTML = "";
  delete (document as { elementFromPoint?: unknown }).elementFromPoint;
});

describe("stream surface helpers", () => {
  it("finds the cuaVideo handler only when the host registers it", () => {
    expect(nativeVideoHandler({})).toBeNull();
    expect(nativeVideoHandler({ webkit: { messageHandlers: { cua: { postMessage() {} } } } })).toBeNull();
    const handler = { postMessage() {} };
    expect(nativeVideoHandler({ webkit: { messageHandlers: { cuaVideo: handler } } })).toBe(handler);
  });

  it("rounds rects, intersects them and skips unchanged reports", () => {
    expect(roundRect({ x: 10.4, y: 20.6, width: 99.5, height: 50.2 })).toEqual({ x: 10, y: 21, width: 100, height: 50 });
    expect(intersect({ x: 0, y: 0, width: 10, height: 10 }, { x: 5, y: 5, width: 10, height: 10 })).toEqual({ x: 5, y: 5, width: 5, height: 5 });
    expect(intersect({ x: 0, y: 0, width: 10, height: 10 }, { x: 10, y: 0, width: 5, height: 5 })).toBeNull();
    const a = surface();
    expect(sameSurface(null, a)).toBe(false);
    expect(sameSurface(a, { ...a })).toBe(true);
    expect(sameSurface(a, { ...a, rect: { ...a.rect, y: 3 } })).toBe(false);
    expect(sameSurface(a, { ...a, clip: null })).toBe(false);
    expect(sameSurface(a, { ...a, occluded: true })).toBe(false);
    expect(sameSurface(a, { ...a, visible: false })).toBe(false);
    expect(sameSurface(a, { ...a, tier: "tile" })).toBe(false);
  });

  it("clips a slot to its scroll container and the viewport", () => {
    vi.stubGlobal("innerWidth", 1000);
    vi.stubGlobal("innerHeight", 700);
    const scroller = document.createElement("div");
    scroller.style.overflowY = "auto";
    const slot = document.createElement("div");
    scroller.append(slot);
    document.body.append(scroller);
    box(scroller, { x: 0, y: 40, width: 800, height: 600 });
    // Scrolled half under the container's top edge.
    expect(visiblePart(slot, { x: 100, y: 0, width: 300, height: 100 })).toEqual({ x: 100, y: 40, width: 300, height: 60 });
    // Scrolled out of it.
    expect(visiblePart(slot, { x: 100, y: -200, width: 300, height: 100 })).toBeNull();
    // Past the container's right edge.
    expect(visiblePart(slot, { x: 700, y: 100, width: 300, height: 100 })).toEqual({ x: 700, y: 100, width: 100, height: 100 });
    // Past the viewport's bottom edge.
    box(scroller, { x: 0, y: 40, width: 800, height: 2000 });
    expect(visiblePart(slot, { x: 0, y: 650, width: 100, height: 100 })).toEqual({ x: 0, y: 650, width: 100, height: 50 });
  });

  it("calls a slot occluded when a dialog, menu or toast overlaps it", () => {
    const slot = document.createElement("div");
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.append(slot, dialog);
    const clip = { x: 0, y: 0, width: 400, height: 300 };
    box(dialog, { x: 500, y: 0, width: 100, height: 100 });
    expect(isOccluded(slot, clip)).toBe(false);
    box(dialog, { x: 350, y: 250, width: 100, height: 100 });
    expect(isOccluded(slot, clip)).toBe(true);
    // A toast (marked `data-video-occluder`) too.
    dialog.removeAttribute("role");
    expect(isOccluded(slot, clip)).toBe(false);
    dialog.setAttribute("data-video-occluder", "");
    expect(isOccluded(slot, clip)).toBe(true);
  });

  it("calls a slot occluded when something else is on top of it (a backdrop)", () => {
    const slot = document.createElement("div");
    const inner = document.createElement("span");
    slot.append(inner);
    const backdrop = document.createElement("div");
    document.body.append(slot, backdrop);
    const clip = { x: 0, y: 0, width: 400, height: 300 };
    document.elementFromPoint = () => inner;
    expect(isOccluded(slot, clip)).toBe(false);
    document.elementFromPoint = (x, y) => (x > 300 && y > 200 ? backdrop : slot);
    expect(isOccluded(slot, clip)).toBe(true);
  });

  it("only streams tiles of running Spaces that can stream their desktop", () => {
    const base = { id: "a", name: "a", status: "running" } as unknown as Space;
    expect(canStreamTile(base)).toBe(true);
    expect(canStreamTile({ ...base, power: { off: true } } as unknown as Space)).toBe(false);
    expect(canStreamTile({ ...base, status: "provisioning" } as Space)).toBe(false);
    const sdk = { features: ["desktop_stream"], spacesdVersion: "1", reachable: true };
    expect(canStreamTile({ ...base, sdk })).toBe(true);
    expect(canStreamTile({ ...base, sdk: { ...sdk, reachable: false } })).toBe(false);
    expect(canStreamTile({ ...base, sdk: { ...sdk, features: [] } })).toBe(false);
  });
});

describe("VideoSlotReporter", () => {
  it("batches what changed, removes slots and relays the host's state and focus", () => {
    vi.stubGlobal("innerWidth", 1000);
    vi.stubGlobal("innerHeight", 700);
    const posted: StreamSurfaceMessage[] = [];
    const reporter = new VideoSlotReporter({ postMessage: (m) => posted.push(m) });
    const tile = document.createElement("div");
    const viewer = document.createElement("div");
    document.body.append(tile, viewer);
    document.elementFromPoint = () => null;
    box(tile, { x: 10, y: 20, width: 232, height: 145 });
    box(viewer, { x: 10, y: 200, width: 640, height: 400 });
    const tileStatus: SlotStatus[] = [];
    const viewerStatus: SlotStatus[] = [];
    const stopTile = reporter.register(tile, { spaceId: "a", tier: "tile", interactive: false, radius: 8 }, (s) => tileStatus.push(s));
    const stopViewer = reporter.register(viewer, { spaceId: "b", tier: "full", interactive: true, radius: 12, inset: 1 }, (s) =>
      viewerStatus.push(s),
    );
    const viewerId = reporter.idOf(viewer)!;
    expect(posted).toHaveLength(2);
    const second = posted[1];
    if (second?.type !== "surfaces") throw new Error("expected surfaces");
    expect(second.update).toEqual([
      {
        surfaceId: viewerId,
        spaceId: "b",
        tier: "full",
        interactive: true,
        rect: { x: 11, y: 201, width: 638, height: 398 },
        clip: { x: 11, y: 201, width: 638, height: 398 },
        radius: 11,
        occluded: false,
        visible: true,
      },
    ]);

    // Nothing moved: nothing sent.
    reporter.flush();
    expect(posted).toHaveLength(2);

    // A scroll moves the tile only.
    box(tile, { x: 10, y: -10, width: 232, height: 145 });
    reporter.flush();
    const moved = posted[2];
    if (moved?.type !== "surfaces") throw new Error("expected surfaces");
    expect(moved.update.map((u) => [u.spaceId, u.rect.y, u.clip?.y])).toEqual([["a", -10, 0]]);

    // The host says the viewer is live, then that keys go to it.
    hostEvent("video.surface", { surfaceId: viewerId, state: "live" });
    hostEvent("video.focus", { surfaceId: viewerId });
    expect(viewerStatus.at(-1)).toEqual({ phase: "live", focused: true, reason: undefined });
    expect(tileStatus).toEqual([]);
    reporter.focus(null);
    expect(posted.at(-1)).toEqual({ type: "focus", surfaceId: null });

    stopViewer();
    expect(posted.at(-1)).toEqual({ type: "surfaces", update: [], remove: [viewerId] });
    stopTile();
    // Stopped: host events go nowhere.
    hostEvent("video.surface", { surfaceId: viewerId, state: "failed" });
    expect(viewerStatus.at(-1)?.phase).toBe("live");
  });
});

describe("<StreamSurface> native video", () => {
  let posted: StreamSurfaceMessage[];

  beforeEach(() => {
    posted = [];
  });

  const mount = (props: { canStream?: boolean; children?: React.ReactNode } = {}) =>
    render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={noCore}>
        <StreamSurface spaceId="sp-1" os="macos" canStream={props.canStream ?? true} previewText="Not reachable">
          {props.children}
        </StreamSurface>
      </BridgeProvider>,
    );

  const hostDrawsVideo = () => {
    (window as Win).webkit = { messageHandlers: { cuaVideo: { postMessage: (m: StreamSurfaceMessage) => posted.push(m) } } };
  };

  const updates = () => posted.flatMap((m) => (m.type === "surfaces" ? m.update : []));

  it("stays the placeholder when the host draws no native video", () => {
    const { container, getByText } = mount();
    expect(getByText("Open window")).toBeTruthy();
    expect(container.querySelector('[data-stream-state="placeholder"]')).toBeTruthy();
    expect(container.querySelector("[data-video-focus]")).toBeNull();
  });

  it("connects, goes live with a focus line, and removes itself on unmount", async () => {
    // The native slots are the Mac app's.
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    hostDrawsVideo();
    const { container, queryByText, getByText, unmount } = mount();
    expect(queryByText("Open window")).toBeNull();
    expect(getByText("Connecting…")).toBeTruthy();
    expect(container.querySelector('[data-stream-surface="sp-1"][data-stream-state="connecting"]')).toBeTruthy();
    await waitFor(() => expect(updates()).not.toHaveLength(0));
    const first = updates()[0];
    if (!first) throw new Error("no surface update");
    expect(first).toMatchObject({ spaceId: "sp-1", tier: "full", interactive: true, radius: 11 });

    act(() => hostEvent("video.surface", { surfaceId: first.surfaceId, state: "live" }));
    expect(container.querySelector('[data-stream-state="live"]')).toBeTruthy();
    expect(getByText("Click the desktop to control it.")).toBeTruthy();
    act(() => hostEvent("video.focus", { surfaceId: first.surfaceId }));
    expect(container.querySelector('[data-video-focus="space"]')?.textContent).toContain("Press and release ⌃⌥ to stop.");
    act(() => getByText("Stop controlling").click());
    expect(posted.at(-1)).toEqual({ type: "focus", surfaceId: null });

    act(() => unmount());
    expect(posted.at(-1)).toEqual({ type: "surfaces", update: [], remove: [first.surfaceId] });
  });

  it("says it could not connect, with Try again (which opens it again) and Open window", async () => {
    hostDrawsVideo();
    const { container, getByText } = mount();
    await waitFor(() => expect(updates()).not.toHaveLength(0));
    const first = updates()[0]!;
    act(() => hostEvent("video.surface", { surfaceId: first.surfaceId, state: "failed", reason: "no stream" }));
    expect(container.querySelector('[data-stream-state="failed"]')).toBeTruthy();
    expect(getByText("Could not connect to the desktop")).toBeTruthy();
    expect(getByText("Open window")).toBeTruthy();
    act(() => getByText("Try again").click());
    await waitFor(() => expect(updates().some((u) => u.surfaceId !== first.surfaceId)).toBe(true));
    expect(posted.some((m) => m.type === "surfaces" && m.remove.includes(first.surfaceId))).toBe(true);
    expect(container.querySelector('[data-stream-state="connecting"]')).toBeTruthy();
  });

  it("says why once when the host could not open a stream, and not for one that failed once open", async () => {
    hostDrawsVideo();
    const { getByText, queryByText } = render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={noCore}>
        <ToastProvider>
          <StreamSurface spaceId="sp-1" os="macos" canStream previewText="Not reachable" />
        </ToastProvider>
      </BridgeProvider>,
    );
    await waitFor(() => expect(updates()).not.toHaveLength(0));
    const first = updates()[0]!;
    act(() => hostEvent("video.surface", { surfaceId: first.surfaceId, state: "failed", reason: "the session ended" }));
    expect(queryByText("Could not open sp-1")).toBeNull();
    act(() => getByText("Try again").click());
    await waitFor(() => expect(updates().some((u) => u.surfaceId !== first.surfaceId)).toBe(true));
    const second = updates().find((u) => u.surfaceId !== first.surfaceId)!;
    act(() => hostEvent("video.surface", { surfaceId: second.surfaceId, state: "failed", reason: "scripted provider failure", opening: true }));
    await waitFor(() => expect(getByText("Could not open sp-1")).toBeTruthy());
    expect(getByText("scripted provider failure")).toBeTruthy();
  });

  /** The demo host, answering `settings.get` with auto-connect off and
   * `spaces.thumbnail` with an image. */
  const host = (over: { autoConnect?: boolean; thumbnail?: string }) => {
    const demo = createDemoAdapter({ latencyMs: 1 });
    return {
      ...demo,
      call: (async (op: string, args: never) => {
        if (op === "spaces.thumbnail") return over.thumbnail ? { url: over.thumbnail, capturedAtMs: 1 } : null;
        const out = await demo.call(op as never, args);
        if (op === "settings.get" && over.autoConnect !== undefined) {
          const snap = out as { values: object };
          return { ...snap, values: { ...snap.values, autoConnect: over.autoConnect } };
        }
        return out;
      }) as typeof demo.call,
    };
  };

  it("waits for Connect while auto-connect is off, then connects", async () => {
    hostDrawsVideo();
    const { findByRole, container } = render(
      <BridgeProvider adapter={host({ autoConnect: false })} core={noCore}>
        <StreamSurface spaceId="sp-1" os="macos" canStream previewText="Not reachable" />
      </BridgeProvider>,
    );
    const connect = await findByRole("button", { name: "Connect" });
    await new Promise((r) => setTimeout(r, 20));
    expect(updates()).toEqual([]);
    expect(container.querySelector('[data-desktop-cover="connect"]')).toBeTruthy();
    act(() => connect.click());
    await waitFor(() => expect(updates()).not.toHaveLength(0));
    expect(container.querySelector('[data-desktop-cover="connecting"]')?.textContent).toContain("Connecting…");
  });

  it("shows the Space's thumbnail blurred behind Connecting…", async () => {
    hostDrawsVideo();
    const url = "data:image/jpeg;base64,AAAA";
    const { container } = render(
      <BridgeProvider adapter={host({ thumbnail: url })} core={noCore}>
        <StreamSurface spaceId="local:design-review" os="macos" canStream previewText="Not reachable" />
      </BridgeProvider>,
    );
    await waitFor(() => expect(container.querySelector("[data-cover-thumbnail]")?.getAttribute("src")).toBe(url));
    expect(container.querySelector("[data-cover-thumbnail]")?.className).toContain("blur");
    expect(container.querySelector('[data-desktop-cover="connecting"]')).toBeTruthy();
  });

  it("does not stream a Space that cannot stream, or one showing other content", async () => {
    hostDrawsVideo();
    mount({ canStream: false });
    cleanup();
    mount({ children: <p>Starting…</p> });
    await new Promise((r) => setTimeout(r, 20));
    expect(posted).toEqual([]);
  });
});

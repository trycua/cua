// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore } from "@/bridge/__tests__/testCore";
import { BridgeProvider, type Space } from "@/bridge";
import { CONNECT_TIMEOUT_MS, CONNECT_TIMEOUT_REASON, StreamSurface } from "@/components/space-detail/stream-surface";
import { ToastProvider } from "@/components/ui/toast";
import { canStreamTile } from "@/components/video/tile-video";
import { resetVideoSlotLoader } from "@/components/video/use-video-slot";
import { nativeVideoHandler, type StreamSurfaceMessage, type SurfaceState } from "@/lib/stream-surface";
import { intersect, isOccluded, resetVideoSlots, roundRect, sameSurface, videoHoles, visiblePart, VideoSlotReporter, type SlotStatus } from "@/lib/video-slots";

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
  vi.useRealTimers();
  cleanup();
  delete (window as Win).webkit;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  resetVideoSlots();
  // jsdom has neither: tests stub them per case.
  delete (document as { elementsFromPoint?: unknown }).elementsFromPoint;
  delete (document as { elementFromPoint?: unknown }).elementFromPoint;
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

  it("calls a slot occluded when a dialog overlaps it, and cuts holes for menus, popovers and toasts", () => {
    const slot = document.createElement("div");
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.append(slot, dialog);
    const clip = { x: 0, y: 0, width: 400, height: 300 };
    box(dialog, { x: 500, y: 0, width: 100, height: 100 });
    expect(isOccluded(slot, clip)).toBe(false);
    box(dialog, { x: 350, y: 250, width: 100, height: 100 });
    expect(isOccluded(slot, clip)).toBe(true);
    expect(videoHoles(slot, clip)).toEqual([]);
    // A menu (or a toast, marked `data-video-occluder`) over it: a hole, clipped to the slot; the video stays.
    for (const mark of [["role", "menu"], ["data-video-occluder", ""]] as const) {
      dialog.removeAttribute("role");
      dialog.removeAttribute("data-video-occluder");
      dialog.setAttribute(mark[0], mark[1]);
      expect(isOccluded(slot, clip)).toBe(false);
      expect(videoHoles(slot, clip)).toEqual([{ x: 350, y: 250, width: 50, height: 50 }]);
    }
  });

  // CUA-2641: the profile card (a Popover, role "dialog", marked as a hole)
  // counted as a dialog and hid the whole desktop while it was open.
  it("never hides a viewer or a tile for a dialog-role popup that is a hole (the profile card)", () => {
    const slot = document.createElement("div");
    const card = document.createElement("div");
    card.setAttribute("role", "dialog");
    card.setAttribute("data-video-occluder", "");
    document.body.append(slot, card);
    box(card, { x: 100, y: 100, width: 200, height: 120 });
    const clip = { x: 0, y: 0, width: 400, height: 300 };
    for (const keep of [false, true]) {
      expect(isOccluded(slot, clip, document, 0, keep)).toBe(false);
      expect(videoHoles(slot, clip, document, keep)).toEqual([{ x: 100, y: 100, width: 200, height: 120 }]);
    }
  });

  // CUA-2641: a dialog-role popup over the Space viewer: the video stays
  // drawn with a hole where it is, a dimmed layer is looked through, and a
  // Space tile still hides. (A modal dialog, the dimmed layer open, hides the
  // viewer too: `VideoSlotReporter.measure`, next test.)
  it("keeps the Space viewer drawn with a hole under a dialog-role popup, and still hides a tile", () => {
    const slot = document.createElement("div");
    const inner = document.createElement("span");
    slot.append(inner);
    const dim = document.createElement("div");
    dim.setAttribute("data-window-dim", "");
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.append(slot, dim, dialog);
    box(dialog, { x: 100, y: 60, width: 200, height: 160 });
    const clip = { x: 0, y: 0, width: 400, height: 300 };
    document.elementsFromPoint = (x, y) => (x > 100 && x < 300 && y > 60 && y < 220 ? [dialog, dim, inner, slot] : [dim, inner, slot]);
    expect(isOccluded(slot, clip, document, 0, true)).toBe(false);
    expect(videoHoles(slot, clip, document, true)).toEqual([{ x: 100, y: 60, width: 200, height: 160 }]);
    // A tile keeps the old rule: the dialog hides it.
    expect(isOccluded(slot, clip, document, 0, false)).toBe(true);
    expect(videoHoles(slot, clip, document, false)).toEqual([]);
  });

  // The owner's report: with a live desktop in the Space, a modal ("Connect Anthropic") opened
  // from the Connect card and the native video drew over the bottom edge of the dialog, and
  // the desktop hid the local cursor on the way to it. A native layer sits above the web view:
  // the page's dimmed layer and the dialog cannot cover it, so the video goes while a modal is up.
  it("hides the viewer, takes no input and gives back the keys while a modal dialog is open, and restores all when it closes", () => {
    vi.stubGlobal("innerWidth", 1000);
    vi.stubGlobal("innerHeight", 700);
    const posted: StreamSurfaceMessage[] = [];
    const reporter = new VideoSlotReporter({ postMessage: (m) => posted.push(m) });
    const viewer = document.createElement("div");
    const tile = document.createElement("div");
    document.body.append(viewer, tile);
    box(viewer, { x: 10, y: 200, width: 640, height: 400 });
    box(tile, { x: 700, y: 20, width: 232, height: 145 });
    document.elementFromPoint = () => viewer;
    document.elementsFromPoint = () => [viewer];
    const stop = [
      reporter.register(viewer, { spaceId: "b", tier: "full", interactive: true, radius: 12 }, () => {}),
      reporter.register(tile, { spaceId: "a", tier: "tile", interactive: false, radius: 8 }, () => {}),
    ];
    const last = (space: string) => {
      const m = [...posted].reverse().find((x) => x.type === "surfaces" && x.update.some((u) => u.spaceId === space)) as Extract<StreamSurfaceMessage, { type: "surfaces" }>;
      return m.update.find((u) => u.spaceId === space)!;
    };
    const focusPosts = () => posted.filter((m) => m.type === "focus");
    expect(last("b")).toMatchObject({ interactive: true, occluded: false });
    // The desktop has the keyboard when the dialog opens (the host says so).
    window.dispatchEvent(new CustomEvent("cua:event", { detail: { event: "video.focus", payload: { surfaceId: reporter.idOf(viewer) } } }));
    const dim = document.createElement("div");
    dim.setAttribute("data-window-dim", "");
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.append(dim, dialog);
    box(dialog, { x: 150, y: 260, width: 300, height: 200 });
    document.elementsFromPoint = (x, y) => (x > 150 && x < 450 && y > 260 && y < 460 ? [dialog, dim, viewer] : [dim, viewer]);
    reporter.flush();
    // Hidden (not drawn over the dialog's edge), no pointer, no keys, no holes to leak through.
    expect(last("b")).toMatchObject({ interactive: false, occluded: true, holes: [] });
    expect(last("a")).toMatchObject({ occluded: true });
    expect(focusPosts()).toEqual([{ type: "focus", surfaceId: null }]);
    // Once per dialog: measuring again does not ask again.
    reporter.flush();
    expect(focusPosts()).toHaveLength(1);
    // Closed (the dialog starts to go: its dim is marked ending): the viewer takes input and draws again.
    dim.setAttribute("data-ending-style", "");
    reporter.flush();
    expect(last("b")).toMatchObject({ interactive: true, occluded: false });
    dim.remove();
    dialog.remove();
    document.elementsFromPoint = () => [viewer];
    reporter.flush();
    expect(last("b")).toMatchObject({ interactive: true, occluded: false, holes: [] });
    // The next dialog takes the keys again if the desktop has them.
    window.dispatchEvent(new CustomEvent("cua:event", { detail: { event: "video.focus", payload: { surfaceId: reporter.idOf(viewer) } } }));
    dim.removeAttribute("data-ending-style");
    document.body.append(dim);
    reporter.flush();
    expect(focusPosts()).toHaveLength(2);
    dim.remove();
    stop.forEach((f) => f());
  });

  it("still cuts a hole, not the whole video, for popovers and menus over the viewer (they are not modal)", () => {
    vi.stubGlobal("innerWidth", 1000);
    vi.stubGlobal("innerHeight", 700);
    const posted: StreamSurfaceMessage[] = [];
    const reporter = new VideoSlotReporter({ postMessage: (m) => posted.push(m) });
    const viewer = document.createElement("div");
    document.body.append(viewer);
    box(viewer, { x: 10, y: 200, width: 640, height: 400 });
    document.elementFromPoint = () => viewer;
    document.elementsFromPoint = () => [viewer];
    const stop = reporter.register(viewer, { spaceId: "b", tier: "full", interactive: true, radius: 12 }, () => {});
    const card = document.createElement("div");
    card.setAttribute("role", "dialog");
    card.setAttribute("data-video-occluder", "");
    document.body.append(card);
    box(card, { x: 100, y: 300, width: 200, height: 120 });
    reporter.flush();
    const m = [...posted].reverse().find((x) => x.type === "surfaces") as Extract<StreamSurfaceMessage, { type: "surfaces" }>;
    expect(m.update[0]).toMatchObject({ interactive: true, occluded: false, holes: [{ x: 100, y: 300, width: 200, height: 120 }] });
    card.remove();
    stop();
  });

  // Regression: the Space header's "⋯" menu turned the desktop black. Base
  // UI puts a transparent full-page click catcher (`data-base-ui-inert`)
  // under an open menu; it hit every sample point, so the whole video hid.
  it("looks through transparent click catchers: an open menu never blacks out the video", () => {
    const slot = document.createElement("div");
    const inner = document.createElement("span");
    slot.append(inner);
    const catcher = document.createElement("div");
    catcher.setAttribute("data-base-ui-inert", "");
    const menu = document.createElement("div");
    menu.setAttribute("role", "menu");
    document.body.append(slot, catcher, menu);
    box(menu, { x: 300, y: -40, width: 160, height: 60 });
    const clip = { x: 0, y: 0, width: 400, height: 300 };
    document.elementsFromPoint = (x, y) => (x > 300 && y < 20 ? [menu, catcher, inner, slot] : [catcher, inner, slot]);
    expect(isOccluded(slot, clip)).toBe(false);
    expect(videoHoles(slot, clip)).toEqual([{ x: 300, y: 0, width: 100, height: 20 }]);
    // The modal menu made the page `inert`: hit-testing skips the slot and
    // reaches its ancestors, which paint nothing over it.
    document.elementsFromPoint = () => [catcher, document.body, document.documentElement];
    expect(isOccluded(slot, clip)).toBe(false);
    // Something that paints over it still hides it.
    const sheet = document.createElement("div");
    document.body.append(sheet);
    document.elementsFromPoint = () => [sheet, catcher, inner, slot];
    expect(isOccluded(slot, clip)).toBe(true);
  });

  it("cuts a hole for the viewer's own key bar, but not for other UI inside the slot", () => {
    const slot = document.createElement("div");
    const bar = document.createElement("div");
    bar.setAttribute("data-video-hole", "");
    const tip = document.createElement("div");
    tip.setAttribute("role", "tooltip");
    slot.append(bar, tip);
    document.body.append(slot);
    box(bar, { x: 100, y: 260, width: 200, height: 30 });
    box(tip, { x: 10, y: 10, width: 50, height: 20 });
    expect(videoHoles(slot, { x: 0, y: 0, width: 400, height: 300 })).toEqual([{ x: 100, y: 260, width: 200, height: 30 }]);
  });

  it("cuts a tile's own checkbox and power out of its video while they show, never another tile's", () => {
    const tile = (name: string) => {
      const scope = document.createElement("div");
      scope.setAttribute("data-video-hole-scope", "");
      const picture = document.createElement("div");
      const check = document.createElement("button");
      check.setAttribute("data-video-hole", "");
      const power = document.createElement("button");
      power.setAttribute("data-video-hole", "");
      power.style.opacity = "0";
      scope.append(picture, check, power);
      scope.dataset.name = name;
      document.body.append(scope);
      return { picture, check, power };
    };
    const a = tile("a");
    const b = tile("b");
    const clip = { x: 0, y: 0, width: 320, height: 200 };
    box(a.picture, clip);
    box(a.check, { x: 8, y: 8, width: 16, height: 16 });
    box(a.power, { x: 288, y: 8, width: 24, height: 24 });
    // The next tile's controls overlap this clip here only to prove scope is what decides.
    box(b.check, { x: 40, y: 8, width: 16, height: 16 });
    // Power is hidden until hover (opacity 0): only the checkbox is cut.
    expect(videoHoles(a.picture, clip)).toEqual([{ x: 8, y: 8, width: 16, height: 16 }]);
    // Hover shows it.
    a.power.style.opacity = "1";
    expect(videoHoles(a.picture, clip)).toEqual([
      { x: 8, y: 8, width: 16, height: 16 },
      { x: 288, y: 8, width: 24, height: 24 },
    ]);
    // Hidden by an ancestor in the tile: no hole.
    a.check.parentElement!.style.opacity = "0";
    expect(videoHoles(a.picture, clip)).toEqual([]);
    // A tile's control is not something on top of its video.
    a.check.parentElement!.style.opacity = "1";
    document.elementsFromPoint = (x, y) => (x < 30 && y < 30 ? [a.check, a.picture] : [a.picture]);
    expect(isOccluded(a.picture, clip, document, 8)).toBe(false);
  });

  it("measures holes again on hover and focus, which change no DOM", () => {
    const sent: StreamSurfaceMessage[] = [];
    const frames: FrameRequestCallback[] = [];
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => (frames.push(cb), frames.length));
    const reporter = new VideoSlotReporter({ postMessage: (m) => void sent.push(m) });
    const scope = document.createElement("div");
    scope.setAttribute("data-video-hole-scope", "");
    const picture = document.createElement("div");
    const power = document.createElement("button");
    power.setAttribute("data-video-hole", "");
    power.style.opacity = "0";
    scope.append(picture, power);
    document.body.append(scope);
    box(picture, { x: 0, y: 0, width: 320, height: 200 });
    box(power, { x: 288, y: 8, width: 24, height: 24 });
    const stop = reporter.register(picture, { spaceId: "a", tier: "tile", interactive: false, radius: 8 }, () => {});
    const last = () => (sent.at(-1) as Extract<StreamSurfaceMessage, { type: "surfaces" }>).update[0]!;
    expect(last().holes).toEqual([]);
    power.style.opacity = "1";
    window.dispatchEvent(new Event("pointerover"));
    frames.splice(0).forEach((f) => f(0));
    expect(last().holes).toEqual([{ x: 288, y: 8, width: 24, height: 24 }]);
    stop();
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
    document.elementsFromPoint = () => [];
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
        holes: [],
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
    expect(first).toMatchObject({ spaceId: "sp-1", tier: "full", interactive: true, radius: 8 });

    act(() => hostEvent("video.surface", { surfaceId: first.surfaceId, state: "live" }));
    expect(container.querySelector('[data-stream-state="live"]')).toBeTruthy();
    // Nothing under the stream: no hint line, no Open in window (the header's PiP button replaces both).
    expect(queryByText("Click the desktop to control it.")).toBeNull();
    expect(queryByText("Open in window")).toBeNull();
    expect(container.querySelector("[data-video-focus]")).toBeNull();
    // With the keyboard: a bar over the video's bottom edge (a hole in the video) says how to stop.
    act(() => hostEvent("video.focus", { surfaceId: first.surfaceId }));
    const bar = container.querySelector('[data-stream-surface="sp-1"] [data-video-focus="space"]');
    expect(bar?.textContent).toContain("Press and release ⌃⌥ to stop.");
    expect(bar?.hasAttribute("data-video-hole")).toBe(true);
    act(() => getByText("Stop controlling").click());
    expect(posted.at(-1)).toEqual({ type: "focus", surfaceId: null });

    act(() => unmount());
    expect(posted.at(-1)).toEqual({ type: "surfaces", update: [], remove: [first.surfaceId] });
  });

  // Regression: closing picture in picture showed "Could not connect to the
  // desktop" before the inline desktop came back. The slot that comes back
  // must start at connecting: no failure or timeout left from the slot (or
  // the wait) before it.
  it("comes back from picture in picture without the old slot's failure or timeout", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    hostDrawsVideo();
    const tree = (children?: React.ReactNode) => (
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={noCore}>
        <StreamSurface spaceId="sp-1" os="linux" canStream previewText="Not reachable">
          {children}
        </StreamSurface>
      </BridgeProvider>
    );
    const { container, rerender, queryByText } = render(tree());
    await waitFor(() => expect(updates()).not.toHaveLength(0));
    // The first picture was slow: the wait ran out once.
    act(() => void vi.advanceTimersByTime(CONNECT_TIMEOUT_MS));
    expect(container.querySelector('[data-stream-state="failed"]')).toBeTruthy();
    const first = updates()[0]!;
    act(() => hostEvent("video.surface", { surfaceId: first.surfaceId, state: "live" }));
    expect(container.querySelector('[data-stream-state="live"]')).toBeTruthy();
    // Popped out: the page's "Playing in a floating window" replaces the stream.
    rerender(tree(<p>Playing in a floating window</p>));
    expect(posted.some((m) => m.type === "surfaces" && m.remove.includes(first.surfaceId))).toBe(true);
    // Popped back in: connecting at once, never the old failure.
    rerender(tree());
    expect(container.querySelector('[data-stream-state="failed"]')).toBeNull();
    expect(queryByText("Could not connect to the desktop")).toBeNull();
    await waitFor(() => expect(updates().some((u) => u.surfaceId !== first.surfaceId)).toBe(true));
    const back = updates().at(-1)!;
    expect(container.querySelector('[data-stream-state="connecting"]')).toBeTruthy();
    act(() => hostEvent("video.surface", { surfaceId: back.surfaceId, state: "live" }));
    expect(container.querySelector('[data-stream-state="live"]')).toBeTruthy();
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

  // Regression: the viewer waited on "Loading the desktop…" / "Connecting…"
  // forever. With no picture after CONNECT_TIMEOUT_MS it says it could not
  // connect, why, and offers one action (Try again); a late picture still shows.
  it("never waits silently: no picture within the timeout says why, with Try again", async () => {
    expect(CONNECT_TIMEOUT_MS).toBeGreaterThanOrEqual(15_000);
    expect(CONNECT_TIMEOUT_MS).toBeLessThanOrEqual(30_000);
    vi.useFakeTimers({ shouldAdvanceTime: true });
    hostDrawsVideo();
    const { container, getByText, queryByText } = mount();
    await waitFor(() => expect(updates()).not.toHaveLength(0));
    const first = updates()[0]!;
    act(() => void vi.advanceTimersByTime(CONNECT_TIMEOUT_MS - 1_000));
    expect(container.querySelector('[data-stream-state="connecting"]')).toBeTruthy();
    expect(queryByText("Could not connect to the desktop")).toBeNull();
    act(() => void vi.advanceTimersByTime(1_000));
    expect(container.querySelector('[data-stream-state="failed"]')).toBeTruthy();
    expect(getByText("Could not connect to the desktop")).toBeTruthy();
    expect(container.querySelector("[data-cover-reason]")?.textContent).toBe(CONNECT_TIMEOUT_REASON);
    // One action: Try again (no second button for a stall).
    expect(container.querySelectorAll("[data-desktop-cover] button")).toHaveLength(1);
    expect(getByText("Try again")).toBeTruthy();
    // A picture that arrives late still shows.
    act(() => hostEvent("video.surface", { surfaceId: first.surfaceId, state: "live" }));
    expect(container.querySelector('[data-stream-state="live"]')).toBeTruthy();
    expect(container.querySelector("[data-desktop-cover]")).toBeNull();
  });

  it("starts the wait over on Try again after a timeout", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    hostDrawsVideo();
    const { container, getByText } = mount();
    await waitFor(() => expect(updates()).not.toHaveLength(0));
    const first = updates()[0]!;
    act(() => void vi.advanceTimersByTime(CONNECT_TIMEOUT_MS));
    expect(container.querySelector('[data-stream-state="failed"]')).toBeTruthy();
    act(() => getByText("Try again").click());
    await waitFor(() => expect(updates().some((u) => u.surfaceId !== first.surfaceId)).toBe(true));
    expect(container.querySelector('[data-stream-state="connecting"]')).toBeTruthy();
    act(() => void vi.advanceTimersByTime(CONNECT_TIMEOUT_MS - 1_000));
    expect(container.querySelector('[data-stream-state="connecting"]')).toBeTruthy();
    act(() => void vi.advanceTimersByTime(1_000));
    expect(container.querySelector('[data-stream-state="failed"]')).toBeTruthy();
  });

  it("says why once when the host could not open a stream, and not for one that failed once open", async () => {
    hostDrawsVideo();
    const { getByText, getAllByText, queryByText } = render(
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
    // In the toast, and on the cover under "Could not connect".
    expect(getAllByText("scripted provider failure")).toHaveLength(2);
    expect(document.querySelector("[data-cover-reason]")?.textContent).toBe("scripted provider failure");
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

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_PREFS, DOCK_MIN_WIDTH, SPACE_LAYOUT_STORAGE_KEY } from "@/lib/space-layout";
import { LAYOUT_CHANGE_EVENT } from "@/lib/stream-surface";
import { useSpaceLayoutStore } from "@/stores/space-layout";

import { CallSplit, DEFAULT_SPLIT, DIVIDER_HIT_PX } from "./call-split";

class FakeResizeObserver {
  observe() {}
  disconnect() {}
}

const W = 1000;
const H = 800;

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  localStorage.clear();
  useSpaceLayoutStore.setState({ ...DEFAULT_PREFS });
  localStorage.clear();
  HTMLElement.prototype.setPointerCapture = () => {};
  HTMLElement.prototype.releasePointerCapture = () => {};
  HTMLElement.prototype.hasPointerCapture = () => true;
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** Renders the split in a column `width` by `height`. */
function setup(width = W, height = H) {
  const view = render(
    <CallSplit stage={<div data-testid="stage" />} below={<div data-testid="below" />} leading={<button type="button">Thread view</button>} />,
  );
  const root = view.container.querySelector<HTMLElement>("[data-call-split]")!;
  Object.defineProperty(root, "clientWidth", { configurable: true, value: width });
  Object.defineProperty(root, "clientHeight", { configurable: true, value: height });
  root.getBoundingClientRect = () => ({ x: 0, y: 0, top: 0, left: 0, width, height, right: width, bottom: height, toJSON: () => ({}) }) as DOMRect;
  // Measure again with the sizes in place (jsdom has no layout).
  act(() => {
    window.dispatchEvent(new Event("resize"));
  });
  return { ...view, root, divider: () => view.container.querySelector<HTMLElement>("[data-call-divider]")! };
}

/** The first measure happens on mount, before jsdom has sizes: remount with sizes known. */
function mountSized(width = W, height = H) {
  const proto = HTMLElement.prototype;
  const cw = Object.getOwnPropertyDescriptor(proto, "clientWidth");
  const ch = Object.getOwnPropertyDescriptor(proto, "clientHeight");
  Object.defineProperty(proto, "clientWidth", { configurable: true, get: () => width });
  Object.defineProperty(proto, "clientHeight", { configurable: true, get: () => height });
  const view = render(<CallSplit stage={<div />} below={<div />} />);
  const root = view.container.querySelector<HTMLElement>("[data-call-split]")!;
  root.getBoundingClientRect = () => ({ x: 0, y: 0, top: 0, left: 0, width, height, right: width, bottom: height, toJSON: () => ({}) }) as DOMRect;
  return {
    ...view,
    root,
    divider: () => view.container.querySelector<HTMLElement>("[data-call-divider]")!,
    restore: () => {
      if (cw) Object.defineProperty(proto, "clientWidth", cw);
      else delete (proto as unknown as Record<string, unknown>).clientWidth;
      if (ch) Object.defineProperty(proto, "clientHeight", ch);
      else delete (proto as unknown as Record<string, unknown>).clientHeight;
    },
  };
}

const down = (el: HTMLElement, x: number, y: number) => fireEvent.pointerDown(el, { button: 0, pointerId: 1, pointerType: "mouse", clientX: x, clientY: y });
const move = (el: HTMLElement, x: number, y: number) => fireEvent.pointerMove(el, { pointerId: 1, clientX: x, clientY: y });
const up = (el: HTMLElement, x: number, y: number) => fireEvent.pointerUp(el, { pointerId: 1, clientX: x, clientY: y });

describe("<CallSplit>", () => {
  void setup;
  // The owner asked for an easier grab: a taller hit area, the same small bar.
  it("takes the drag on a row taller than its bar, with the row-resize cursor", () => {
    const m = mountSized();
    const divider = m.divider();
    expect(DIVIDER_HIT_PX).toBeGreaterThanOrEqual(16);
    expect(divider.className).toContain(`h-${DIVIDER_HIT_PX / 4}`);
    expect(divider.className).toContain("cursor-row-resize");
    const bar = divider.querySelector("[data-call-divider-bar]")!;
    expect(bar.className).toContain("h-1");
    expect(bar.className).toContain("group-hover:");
    m.restore();
  });

  it("follows a drag, remembers the split, and brings it back", () => {
    const m = mountSized();
    expect(m.root.dataset.callSplit).toBe(DEFAULT_SPLIT.toFixed(2));
    down(m.divider(), 500, 416);
    expect(m.divider().querySelector("[data-call-divider-bar]")!.className).toContain("bg-ring");
    move(m.divider(), 500, 560);
    expect(m.root.dataset.callSplit).toBe("0.70");
    // Nothing is saved until the pointer lets go.
    expect(localStorage.getItem(SPACE_LAYOUT_STORAGE_KEY)).toBeNull();
    up(m.divider(), 500, 560);
    expect(JSON.parse(localStorage.getItem(SPACE_LAYOUT_STORAGE_KEY)!).state).toMatchObject({ dock: "top", vSplit: 0.7 });
    m.unmount();
    const again = mountSized();
    expect(again.root.dataset.callSplit).toBe("0.70");
    again.restore();
  });

  it("keeps both panes at their minimum while dragging", () => {
    const m = mountSized();
    down(m.divider(), 500, 416);
    move(m.divider(), 500, 5);
    expect(Number(m.root.dataset.callSplit)).toBeCloseTo(160 / H, 1);
    move(m.divider(), 500, 795);
    expect(Number(m.root.dataset.callSplit)).toBeCloseTo(1 - 200 / H, 1);
    up(m.divider(), 500, 795);
    m.restore();
  });

  it("docks the desktop to the left when the divider is dragged to the left edge, with a preview until it lets go", () => {
    const m = mountSized();
    down(m.divider(), 500, 416);
    move(m.divider(), 400, 416);
    expect(m.container.querySelector("[data-dock-preview]")).toBeNull();
    move(m.divider(), 60, 416);
    const preview = m.container.querySelector<HTMLElement>("[data-dock-preview]")!;
    expect(preview.dataset.dockPreview).toBe("left");
    // The preview is cut out of the native video (four thin bars), not drawn over it.
    expect(preview.querySelectorAll("[data-video-hole]")).toHaveLength(4);
    // Still vertical until it ends.
    expect(m.root.dataset.callDock).toBe("top");
    up(m.divider(), 60, 416);
    expect(m.container.querySelector("[data-dock-preview]")).toBeNull();
    expect(m.root.dataset.callDock).toBe("left");
    expect(m.divider().getAttribute("aria-orientation")).toBe("vertical");
    expect(m.divider().className).toContain("cursor-col-resize");
    expect(JSON.parse(localStorage.getItem(SPACE_LAYOUT_STORAGE_KEY)!).state.dock).toBe("left");
    m.restore();
  });

  it("docks to the right, resizes there, and goes back to the top when dragged up", () => {
    const m = mountSized();
    down(m.divider(), 500, 416);
    move(m.divider(), 960, 416);
    expect(m.container.querySelector("[data-dock-preview]")?.getAttribute("data-dock-preview")).toBe("right");
    up(m.divider(), 960, 416);
    expect(m.root.dataset.callDock).toBe("right");
    // The desktop is on the right: dragging the divider left makes it bigger.
    const before = Number(m.root.dataset.callSplit);
    down(m.divider(), 420, 400);
    move(m.divider(), 300, 400);
    expect(Number(m.root.dataset.callSplit)).toBeGreaterThan(before);
    expect(Number(m.root.dataset.callSplit)).toBeCloseTo(0.7, 1);
    // Toward the top: back to the vertical split.
    move(m.divider(), 300, 20);
    expect(m.container.querySelector("[data-dock-preview]")?.getAttribute("data-dock-preview")).toBe("top");
    up(m.divider(), 300, 20);
    expect(m.root.dataset.callDock).toBe("top");
    m.restore();
  });

  it("does not flicker the preview at the edge of a zone", () => {
    const m = mountSized();
    down(m.divider(), 500, 416);
    move(m.divider(), 100, 416);
    expect(m.container.querySelector("[data-dock-preview]")).not.toBeNull();
    move(m.divider(), 170, 416);
    expect(m.container.querySelector("[data-dock-preview]")).not.toBeNull();
    move(m.divider(), 260, 416);
    expect(m.container.querySelector("[data-dock-preview]")).toBeNull();
    up(m.divider(), 260, 416);
    expect(m.root.dataset.callDock).toBe("top");
    m.restore();
  });

  it("cancels a drag with Escape: nothing changes, nothing is saved", () => {
    const m = mountSized();
    down(m.divider(), 500, 416);
    move(m.divider(), 60, 416);
    expect(m.container.querySelector("[data-dock-preview]")).not.toBeNull();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(m.container.querySelector("[data-dock-preview]")).toBeNull();
    up(m.divider(), 60, 416);
    expect(m.root.dataset.callDock).toBe("top");
    expect(m.root.dataset.callSplit).toBe(DEFAULT_SPLIT.toFixed(2));
    expect(localStorage.getItem(SPACE_LAYOUT_STORAGE_KEY)).toBeNull();
    m.restore();
  });

  it("works with touch and pen as well as the mouse", () => {
    const m = mountSized();
    fireEvent.pointerDown(m.divider(), { pointerId: 2, pointerType: "touch", clientX: 500, clientY: 416 });
    fireEvent.pointerMove(m.divider(), { pointerId: 2, pointerType: "touch", clientX: 500, clientY: 600 });
    fireEvent.pointerUp(m.divider(), { pointerId: 2, pointerType: "touch", clientX: 500, clientY: 600 });
    expect(m.root.dataset.callSplit).toBe("0.75");
    m.restore();
  });

  it("resizes with the arrow keys, Shift for bigger steps, and Home or a double-click for the default", () => {
    const m = mountSized();
    const d = m.divider();
    expect(d.tabIndex).toBe(0);
    fireEvent.keyDown(d, { key: "ArrowDown" });
    expect(m.root.dataset.callSplit).toBe("0.55");
    fireEvent.keyDown(d, { key: "ArrowUp", shiftKey: true });
    expect(m.root.dataset.callSplit).toBe("0.45");
    fireEvent.keyDown(d, { key: "Home" });
    expect(m.root.dataset.callSplit).toBe(DEFAULT_SPLIT.toFixed(2));
    fireEvent.keyDown(d, { key: "ArrowDown", shiftKey: true });
    fireEvent.doubleClick(d);
    expect(m.root.dataset.callSplit).toBe(DEFAULT_SPLIT.toFixed(2));
    m.restore();
  });

  it("uses the arrows along the divider's axis once docked", () => {
    useSpaceLayoutStore.setState({ dock: "left", hSplit: 0.5 });
    const m = mountSized();
    const d = m.divider();
    fireEvent.keyDown(d, { key: "ArrowRight" });
    expect(m.root.dataset.callSplit).toBe("0.53");
    fireEvent.keyDown(d, { key: "ArrowDown" });
    expect(m.root.dataset.callSplit).toBe("0.53");
    cleanup();
    useSpaceLayoutStore.setState({ dock: "right", hSplit: 0.5 });
    const r = mountSized();
    fireEvent.keyDown(r.divider(), { key: "ArrowLeft" });
    expect(r.root.dataset.callSplit).toBe("0.53");
    fireEvent.doubleClick(r.divider());
    expect(r.root.dataset.callSplit).toBe("0.58");
    r.restore();
  });

  it("puts the desktop beside the thread when docked, on the saved side", () => {
    useSpaceLayoutStore.setState({ dock: "right", hSplit: 0.6 });
    const m = mountSized();
    expect(m.root.className).toContain("flex-row-reverse");
    expect(m.root.querySelector<HTMLElement>("[data-call-stage]")!.style.width).toBe("60%");
    m.restore();
  });

  it("falls back to the desktop on top in a narrow window, without a word, and keeps the saved dock", () => {
    useSpaceLayoutStore.setState({ dock: "left" });
    const narrow = mountSized(DOCK_MIN_WIDTH - 20, H);
    expect(narrow.root.dataset.callDock).toBe("top");
    expect(narrow.container.textContent).toBe("");
    // A drag to the edge does not dock there.
    down(narrow.divider(), 300, 416);
    move(narrow.divider(), 20, 416);
    expect(narrow.container.querySelector("[data-dock-preview]")).toBeNull();
    up(narrow.divider(), 20, 416);
    expect(narrow.root.dataset.callDock).toBe("top");
    expect(useSpaceLayoutStore.getState().dock).toBe("left");
    narrow.restore();
  });

  it("tells the video host after each layout change", () => {
    const heard = vi.fn();
    window.addEventListener(LAYOUT_CHANGE_EVENT, heard);
    const m = mountSized();
    heard.mockClear();
    down(m.divider(), 500, 416);
    move(m.divider(), 500, 500);
    expect(heard).toHaveBeenCalled();
    heard.mockClear();
    move(m.divider(), 500, 560);
    expect(heard).toHaveBeenCalledTimes(1);
    up(m.divider(), 500, 560);
    window.removeEventListener(LAYOUT_CHANGE_EVENT, heard);
    m.restore();
  });

  it("keeps the Thread view control on the divider, away from the drag", () => {
    const { container } = setup();
    const lead = container.querySelector<HTMLElement>("[data-call-divider-leading]")!;
    fireEvent.pointerDown(lead.querySelector("button")!, { button: 0, pointerId: 1 });
    expect(container.querySelector("[data-call-divider][data-dragging]")).toBeNull();
  });
});

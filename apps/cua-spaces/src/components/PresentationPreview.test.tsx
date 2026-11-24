// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { presentationPreview, previewFrame, previewStill } from "../model/presentationPreview";
import { PresentationPreview } from "./PresentationPreview";

const stage = (c: HTMLElement) => c.querySelector<HTMLElement>(".obp-picture")!;
const labels = (c: HTMLElement, sel: string) => Array.from(c.querySelectorAll(sel)).map((e) => e.textContent);

function reduceMotion(on: boolean) {
  const original = window.matchMedia;
  window.matchMedia = ((query: string) => ({
    ...original(query),
    matches: on && query.includes("prefers-reduced-motion"),
  })) as typeof window.matchMedia;
  return () => {
    window.matchMedia = original;
  };
}

describe("PresentationPreview", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
    vi.useRealTimers();
  });

  it("draws the notch card from the core's scene: the tab while closed, the real panel when open", () => {
    const scene = presentationPreview(false);
    const closed = render(<PresentationPreview menuBar={false} fixedMs={0} />);
    const s = stage(closed.container);
    expect(s).toHaveAttribute("data-preview", "notch");
    const st = closed.container.querySelector<HTMLElement>(".obp-stage")!;
    expect(st.style.width).toBe(`${scene.width}px`);
    expect(st.style.height).toBe(`${scene.height}px`);
    expect(s.style.height).toBe(`${scene.height}px`);
    expect(closed.container.querySelector(".obp-tab-count")).toHaveTextContent("3");
    expect(closed.container.querySelector<HTMLElement>(".obp-notch-scaled")!.style.opacity).toBe("0");
    closed.unmount();

    const open = render(<PresentationPreview menuBar={false} fixedMs={2400} />);
    const c = open.container;
    expect(labels(c, ".obp-tile-name")).toEqual(scene.notch!.view.tiles.map((t) => t.name));
    expect(c.querySelector(".obp-search")).toHaveTextContent("Search");
    expect(c.querySelectorAll(".obp-button")).toHaveLength(2);
    expect(c.querySelector<HTMLElement>(".obp-notch-scaled")!.style.opacity).toBe("1");
    expect(Number(c.querySelector<SVGElement>(".obp-notch")!.getAttribute("width"))).toBeCloseTo(
      scene.notch!.open.width,
      1,
    );
  });

  it("draws the menu bar card with the core's menu, highlighting the row under the pointer", () => {
    const scene = presentationPreview(true);
    const idle = render(<PresentationPreview menuBar fixedMs={0} />);
    expect(stage(idle.container)).toHaveAttribute("data-preview", "menu-bar");
    expect(idle.container.querySelector(".obp-menu")).toBeNull();
    expect(idle.container.querySelector(".obp-status-highlight")).toBeNull();
    idle.unmount();

    const open = render(<PresentationPreview menuBar fixedMs={2400} />);
    const c = open.container;
    const items = scene.menu!.rows.filter((r) => r.item.id !== "separator").map((r) => r.item.label);
    expect(labels(c, ".obp-menu-label")).toEqual(items);
    expect(items).toEqual(["3 Spaces", "Open Cua Spaces", "New Space…", "Settings…", "Quit Cua Spaces"]);
    expect(labels(c, ".obp-menu-shortcut")).toEqual(["⌘,", "⌘Q"]);
    expect(c.querySelector('[data-highlighted="true"]')).toHaveTextContent("Open Cua Spaces");
    expect(c.querySelector('[data-disabled="true"]')).toHaveTextContent("3 Spaces");
    expect(c.querySelector(".obp-status-highlight")).not.toBeNull();
  });

  it("follows the core's frames: the pointer and the press", () => {
    const f = previewFrame(true, 1350);
    const { container } = render(<PresentationPreview menuBar fixedMs={1350} />);
    const pointer = container.querySelector<SVGElement>(".obp-pointer")!;
    expect(f.pressed).toBe(true);
    expect(pointer.style.transform).toBe(`translate(${f.pointer.x}px, ${f.pointer.y}px) scale(0.85)`);
  });

  it("with reduced motion shows the expanded still and does not animate", () => {
    restore = reduceMotion(true);
    const raf = vi.spyOn(window, "requestAnimationFrame");
    const notch = render(<PresentationPreview menuBar={false} />);
    expect(stage(notch.container)).toHaveAttribute("data-still", "true");
    expect(notch.container.querySelector<HTMLElement>(".obp-notch-scaled")!.style.opacity).toBe("1");
    const menu = render(<PresentationPreview menuBar />);
    const still = previewStill(true);
    expect(menu.container.querySelector(".obp-menu")).not.toBeNull();
    expect(menu.container.querySelector('[data-highlighted="true"]')).toHaveTextContent(
      presentationPreview(true).menu!.rows[still.highlighted!]!.item.label,
    );
    expect(raf).not.toHaveBeenCalled();
    raf.mockRestore();
  });

  it("animates only while the window has focus", () => {
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    const raf = vi.spyOn(window, "requestAnimationFrame");
    render(<PresentationPreview menuBar />);
    expect(raf).not.toHaveBeenCalled();
    focus.mockReturnValue(true);
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(raf).toHaveBeenCalled();
    focus.mockRestore();
    raf.mockRestore();
  });
});

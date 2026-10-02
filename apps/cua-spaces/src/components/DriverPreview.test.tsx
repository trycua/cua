// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { driverPreview, driverPreviewFrame, driverPreviewStill } from "../model/driverPreview";
import { DriverPreview } from "./DriverPreview";

const picture = (c: HTMLElement) => c.querySelector<HTMLElement>(".obp-picture")!;
const ticks = (c: HTMLElement) =>
  Array.from(c.querySelectorAll<HTMLElement>(".obdp-checkbox")).map((b) => Number(b.dataset.checked));
const px = (n: number) => `${n}px`;

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

describe("DriverPreview", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });

  it("draws the core's scene: two windows, the agent cursor between them, the user's pointer on top", () => {
    const scene = driverPreview();
    const { container: c } = render(<DriverPreview fixedMs={0} />);
    expect(picture(c)).toHaveAttribute("data-preview", "driver");
    expect(picture(c).style.height).toBe(px(scene.height));
    const stage = c.querySelector<HTMLElement>(".obp-stage")!;
    expect(stage.style.width).toBe(px(scene.width));
    // Z-order is document order: back window, agent, front window, pointer.
    expect(Array.from(stage.children).map((e) => e.getAttribute("data-window") ?? e.getAttribute("class"))).toEqual([
      "back",
      "obdp-agent",
      "front",
      "obp-pointer",
    ]);
    const back = c.querySelector<HTMLElement>('[data-window="back"]')!;
    expect([back.style.left, back.style.top, back.style.width, back.style.height]).toEqual(
      [scene.back.frame.x, scene.back.frame.y, scene.back.frame.width, scene.back.frame.height].map(px),
    );
    expect(back.querySelectorAll(".obdp-titlebar i")).toHaveLength(3);
    expect(back.querySelectorAll(".obdp-checkbox")).toHaveLength(scene.checkboxes.length);
    expect(back.querySelectorAll(".obdp-bar")).toHaveLength(scene.labels.length);
    const box = back.querySelector<HTMLElement>(".obdp-checkbox")!;
    expect(box.style.left).toBe(px(scene.checkboxes[0]!.x - scene.back.frame.x));
    const front = c.querySelector<HTMLElement>('[data-window="front"]')!;
    expect(front.querySelectorAll(".obdp-bar")).toHaveLength(scene.lines.length);
    // The agent cursor is the driver theme's arrow in its fill.
    const arrow = c.querySelector<SVGPolygonElement>(".obdp-agent-arrow")!;
    expect(arrow.getAttribute("fill")).toBe(scene.agentFill);
    expect(scene.agentFill).toBe("#5EC0E8");
    expect(arrow.getAttribute("points")!.split(" ")).toHaveLength(scene.agentPointer.length);
  });

  it("follows the core's frames: ticks, the selection, both cursors and the click rays", () => {
    const scene = driverPreview();
    const line = scene.lines[scene.selectedLine]!;
    for (const t of [950, 1500, 2600, 3500]) {
      const f = driverPreviewFrame(t);
      const { container: c, unmount } = render(<DriverPreview fixedMs={t} />);
      expect(ticks(c)).toEqual(f.checked);
      const sel = c.querySelector<HTMLElement>(".obdp-selection")!;
      expect(Number(sel.dataset.selection)).toBe(f.selection);
      expect(parseFloat(sel.style.width)).toBeCloseTo(line.width * f.selection, 5);
      const agent = c.querySelector<SVGElement>(".obdp-agent")!;
      expect(agent.style.transform).toBe(`translate(${f.agent.x}px, ${f.agent.y}px)`);
      expect(agent.querySelector("g:last-child")!.getAttribute("transform")).toBe(
        `scale(${f.agentPressed ? 0.85 : 1})`,
      );
      const rays = c.querySelector(".obdp-rays");
      if (f.ripple > 0) {
        expect(rays!.querySelectorAll("line")).toHaveLength(scene.agentRays.length);
        expect(Number(rays!.getAttribute("opacity"))).toBeCloseTo(1 - f.ripple, 5);
      } else {
        expect(rays).toBeNull();
      }
      const pointer = c.querySelector<SVGElement>(".obp-pointer")!;
      expect(pointer.style.transform).toBe(
        `translate(${f.pointer.x}px, ${f.pointer.y}px) scale(${f.pressed ? 0.85 : 1})`,
      );
      unmount();
    }
    // Mid-loop the agent has ticked a box while the user drags.
    expect(driverPreviewFrame(1500).checked[0]).toBe(1);
    expect(driverPreviewFrame(1500).selection).toBeGreaterThan(0);
  });

  it("with reduced motion shows the core's still and does not animate", () => {
    restore = reduceMotion(true);
    const raf = vi.spyOn(window, "requestAnimationFrame");
    const { container: c } = render(<DriverPreview />);
    const still = driverPreviewStill();
    expect(picture(c)).toHaveAttribute("data-still", "true");
    expect(ticks(c)).toEqual(still.checked);
    expect(c.querySelector<SVGElement>(".obdp-agent")!.style.transform).toBe(
      `translate(${still.agent.x}px, ${still.agent.y}px)`,
    );
    expect(c.querySelector(".obdp-rays")).not.toBeNull();
    expect(raf).not.toHaveBeenCalled();
    raf.mockRestore();
  });

  it("animates only while on screen and the window has focus", () => {
    const original = globalThis.IntersectionObserver;
    let report: ((visible: boolean) => void) | undefined;
    globalThis.IntersectionObserver = class {
      constructor(cb: IntersectionObserverCallback) {
        report = (visible) => cb([{ isIntersecting: visible } as IntersectionObserverEntry], this as never);
      }
      observe() {}
      disconnect() {}
    } as unknown as typeof IntersectionObserver;
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    const raf = vi.spyOn(window, "requestAnimationFrame");
    const cancel = vi.spyOn(window, "cancelAnimationFrame");
    render(<DriverPreview />);
    expect(raf).not.toHaveBeenCalled();
    focus.mockReturnValue(true);
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(raf).toHaveBeenCalled();
    act(() => report!(false));
    expect(cancel).toHaveBeenCalled();
    focus.mockRestore();
    raf.mockRestore();
    cancel.mockRestore();
    globalThis.IntersectionObserver = original;
  });
});

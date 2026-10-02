// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { drivePreview, drivePreviewFrame, drivePreviewStill } from "../model/driveMountPreview";
import { DriveMountPreview } from "./DriveMountPreview";

const picture = (c: HTMLElement) => c.querySelector<HTMLElement>(".obp-picture")!;
const px = (n: number) => `${n}px`;
const opacity = (e: Element) => Number((e as HTMLElement).style.opacity);
const arrived = (c: HTMLElement) =>
  Array.from(c.querySelectorAll('[data-window="finder"] .obdmp-file')).map(opacity);

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

describe("DriveMountPreview", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });

  it("draws the core's scene: a Space, a Finder window with the Cua Volume volume, the file in flight on top", () => {
    const scene = drivePreview();
    const { container: c } = render(<DriveMountPreview fixedMs={1200} />);
    expect(picture(c)).toHaveAttribute("data-preview", "drive");
    expect(picture(c).style.height).toBe(px(scene.height));
    const stage = c.querySelector<HTMLElement>(".obp-stage")!;
    expect(stage.style.width).toBe(px(scene.width));
    // Z-order is document order: the Space, the Finder window, the flight.
    expect(
      Array.from(stage.children).map((e) => e.getAttribute("data-window") ?? e.getAttribute("class")),
    ).toEqual(["space", "finder", "obdmp-file obdmp-flight"]);
    const space = c.querySelector<HTMLElement>('[data-window="space"]')!;
    expect([space.style.left, space.style.top, space.style.width, space.style.height]).toEqual(
      [scene.space.frame.x, scene.space.frame.y, scene.space.frame.width, scene.space.frame.height].map(px),
    );
    expect(space.querySelectorAll(".obdp-titlebar i")).toHaveLength(3);
    expect(space.querySelectorAll(".obdmp-file")).toHaveLength(scene.sourceIcons.length);
    expect(space.querySelectorAll(".obdp-bar")).toHaveLength(scene.sourceLabels.length);
    const finder = c.querySelector<HTMLElement>('[data-window="finder"]')!;
    expect(finder.querySelector(".obdmp-sidebar")).not.toBeNull();
    // The sidebar's two places and the three landing rows' name bars.
    expect(finder.querySelectorAll(".obdp-bar")).toHaveLength(scene.places.length + scene.destLabels.length);
    const label = finder.querySelector<HTMLElement>(".obdmp-volume-label")!;
    expect(label.textContent).toBe("Cua Volume");
    expect(label.textContent).toBe(scene.volumeLabel);
    expect(label.style.fontSize).toBe(px(scene.fontSize));
    expect(label.style.left).toBe(px(scene.volumeLabelX - scene.finder.frame.x));
    expect(finder.querySelector(".obdmp-volume-icon")).not.toBeNull();
  });

  it("follows the core's frames: the volume, the file in flight and the landed rows", () => {
    const scene = drivePreview();
    for (const t of [0, 400, 1200, 1700, 3000, 4799]) {
      const f = drivePreviewFrame(t);
      const { container: c, unmount } = render(<DriveMountPreview fixedMs={t} />);
      const volume = c.querySelector<HTMLElement>(".obdmp-volume")!;
      expect(Number(volume.dataset.volume)).toBe(f.volume);
      expect(opacity(volume)).toBeCloseTo(f.volume, 5);
      expect(arrived(c)).toEqual(f.arrived);
      const flight = c.querySelector<HTMLElement>(".obdmp-flight");
      if (f.flight) {
        expect([flight!.style.left, flight!.style.top]).toEqual([px(f.flight.x), px(f.flight.y)]);
        expect(flight!.style.width).toBe(px(scene.sourceIcons[0]!.width));
      } else {
        expect(flight).toBeNull();
      }
      unmount();
    }
    // Mid-loop a file is in the air and the first one has landed.
    expect(drivePreviewFrame(1700).arrived[0]).toBe(1);
    expect(drivePreviewFrame(1700).flight).not.toBeNull();
  });

  it("with reduced motion shows the core's still and does not animate", () => {
    restore = reduceMotion(true);
    const raf = vi.spyOn(window, "requestAnimationFrame");
    const { container: c } = render(<DriveMountPreview />);
    const still = drivePreviewStill();
    expect(picture(c)).toHaveAttribute("data-still", "true");
    expect(arrived(c)).toEqual(still.arrived);
    expect(Number(c.querySelector<HTMLElement>(".obdmp-volume")!.dataset.volume)).toBe(still.volume);
    expect(c.querySelector(".obdmp-flight")).not.toBeNull();
    expect(raf).not.toHaveBeenCalled();
    raf.mockRestore();
  });
});

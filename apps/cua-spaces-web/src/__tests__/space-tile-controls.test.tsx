// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Every tile on the Spaces grid has the same controls: the checkbox (on
// hover, and always while something is picked), power and "⋯". A control
// you may not use stays, disabled, saying why. The controls over the picture
// are cut out of a live tile's native video (macOS draws the live desktop
// over the page), so a running tile with live video shows them too. The
// selected ring sits outside the picture, in room its grid cell keeps, so
// nothing clips it.

import { cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { TILE_CELL } from "@/components/spaces/space-tile";
import { resetVideoSlotLoader } from "@/components/video/use-video-slot";
import type { StreamSurfaceMessage } from "@/lib/stream-surface";
import { resetVideoSlots } from "@/lib/video-slots";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();

const HOST = { failsCreates: false, listsGhost: false };

/** One of each kind of tile in the demo. */
const KINDS = {
  localRunning: "local:design-review",
  remoteRunning: "relay:mac-mini/qa-windows",
  localStopped: "local:agent-sandbox",
  remoteSuspended: "relay:mac-mini/release-checks",
  sharedEditor: "relay:space-sam-qa-shared",
  sharedViewer: "relay:space-priya-checkout",
} as const;

type Win = typeof window & { webkit?: unknown };
let posted: StreamSurfaceMessage[] = [];

beforeEach(() => {
  // The macOS host's native video handler: running tiles register live video slots.
  posted = [];
  (window as Win).webkit = { messageHandlers: { cuaVideo: { postMessage: (m: StreamSurfaceMessage) => void posted.push(m) } } };
  resetVideoSlots();
  resetVideoSlotLoader();
});

afterEach(() => {
  cleanup();
  delete (window as Win).webkit;
  resetVideoSlots();
  resetVideoSlotLoader();
  vi.restoreAllMocks();
});

const tileOf = (id: string) =>
  waitFor(() => {
    const link = document.querySelector<HTMLElement>(`[data-space-id="${id}"]`);
    if (!link) throw new Error(`no tile ${id}`);
    return link.parentElement!;
  });

describe.skipIf(!wasmBuilt)("a Space tile's controls", () => {
  it("every kind of tile has its checkbox, power and ⋯, over the picture as video overlays", async () => {
    await mountApp("/spaces", HOST);
    // The running tiles register live video with the host.
    await waitFor(() => expect(posted.some((m) => m.type === "surfaces" && m.update.some((u) => u.spaceId === KINDS.localRunning))).toBe(true));
    for (const [kind, id] of Object.entries(KINDS)) {
      const tile = await tileOf(id);
      expect(tile.hasAttribute("data-video-hole-scope"), kind).toBe(true);
      const box = tile.querySelector<HTMLElement>(`[data-select="${id}"]`);
      expect(box, `${kind}: checkbox`).not.toBeNull();
      expect(box!.hasAttribute("data-video-hole"), `${kind}: checkbox cut out of video`).toBe(true);
      await waitFor(() => expect(tile.querySelector(`[data-space-more="${id}"]`), `${kind}: ⋯`).not.toBeNull());
      const power = tile.querySelector<HTMLElement>("[data-tile-power]");
      expect(power, `${kind}: power`).not.toBeNull();
      expect(power!.hasAttribute("data-video-hole"), `${kind}: power cut out of video`).toBe(true);
    }
    // The live tile's picture is the video slot.
    const live = await tileOf(KINDS.localRunning);
    expect(live.querySelector("[data-tile-picture]")?.hasAttribute("data-stream-state")).toBe(true);
  });

  it("a Space shared with you to watch shows power disabled, saying why, and turns nothing on", async () => {
    await mountApp("/spaces", HOST);
    const tile = await tileOf(KINDS.sharedViewer);
    const power = tile.querySelector<HTMLElement>("[data-tile-power]")!;
    expect(power.getAttribute("aria-disabled")).toBe("true");
    expect(power.title).toMatch(/can view this Space but not turn it on or off/);
    fireEvent.click(power);
    expect(tile.querySelector("[data-space-id]")?.getAttribute("data-state")).toBe("stopped");
    // One you may use is enabled.
    const editor = (await tileOf(KINDS.sharedEditor)).querySelector<HTMLElement>("[data-tile-power]")!;
    expect(editor.hasAttribute("aria-disabled")).toBe(false);
  });

  it("the selected ring is drawn outside the picture, and nothing around it clips it", async () => {
    await mountApp("/spaces", HOST);
    const tile = await tileOf(KINDS.localRunning);
    fireEvent.click(tile.querySelector<HTMLElement>(`[data-select="${KINDS.localRunning}"]`)!);
    await waitFor(() => expect(tile.querySelector("[data-space-id]")?.hasAttribute("data-selected")).toBe(true));
    const picture = tile.querySelector<HTMLElement>("[data-tile-picture]")!;
    const classes = picture.className.split(/\s+/);
    expect(classes).toContain("ring-2");
    expect(classes).toContain("ring-brand");
    expect(classes).toContain("ring-offset-2");
    // Inside the picture a live tile's native video would cover it.
    expect(classes).not.toContain("ring-inset");
    // Every ancestor that clips what it paints (overflow, content-visibility,
    // paint containment) keeps at least the ring's 4 px of room around it.
    const clips = /^(overflow(-[xy])?-(hidden|clip|auto|scroll)|\[content-visibility:auto\]|\[contain:.*paint.*\]|contain-paint)$/;
    const grid = tile.closest("ul")!;
    for (let a = tile.parentElement; a && a !== grid; a = a.parentElement) {
      const tokens = a.className.toString().split(/\s+/);
      if (!tokens.some((t) => clips.test(t))) continue;
      expect(tokens, `${a.tagName} clips the ring`).toEqual(expect.arrayContaining(["p-1", "-m-1"]));
    }
    expect(tile.closest("li")!.className).toBe(TILE_CELL);
  });
});

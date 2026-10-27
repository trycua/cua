// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Spaces grid at scale: picking one Space redraws that tile, not the
// grid. With 3,000 Spaces every pick redrew 3,000 tiles (about 480 ms on a
// laptop; scripts/scale-bench.mjs measures it in Chromium). Counts each
// tile's draws on the real routes, the wasm core and the demo host.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

const draws = new Map<string, number>();
vi.mock("@/components/spaces/space-tile", async (actual) => {
  const real = await actual<typeof import("@/components/spaces/space-tile")>();
  return {
    ...real,
    SpaceTile: (props: Parameters<typeof real.SpaceTile>[0]) => {
      draws.set(props.item.space.id, (draws.get(props.item.space.id) ?? 0) + 1);
      return real.SpaceTile(props);
    },
  };
});

afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("the Spaces grid", () => {
  it("redraws only the tile picked, and the first pick doesn't redraw the rest", { timeout: 30_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { extraSpaces: 60 } });
    await waitFor(() => expect(document.querySelectorAll("[data-space-id]").length).toBeGreaterThan(60));
    // Let the page settle (thumbnails, resources, the first-run reads).
    await act(() => new Promise((r) => setTimeout(r, 800)));
    draws.clear();
    const box = document.querySelector<HTMLElement>('[data-select="local:scale-7"]')!;
    fireEvent.click(box);
    await waitFor(() => expect(document.querySelector("[data-selection-bar]")).not.toBeNull());
    expect(document.querySelector('[data-select="local:scale-7"]')?.getAttribute("aria-checked")).toBe("true");
    const redrawn = [...draws.keys()];
    expect(redrawn).toContain("local:scale-7");
    expect(redrawn.length, `redrew ${redrawn.length} tiles: ${redrawn.slice(0, 8).join(", ")}…`).toBeLessThanOrEqual(3);
  });

  it("draws one screen of a long grid, not every Space (jsdom has no width, so the grid draws its first rows)", { timeout: 30_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { extraSpaces: 400 } });
    await waitFor(() => expect(document.querySelectorAll("[data-space-id]").length).toBeGreaterThan(0));
    const drawn = document.querySelectorAll("[data-space-id]").length;
    expect(drawn, `${drawn} of 410 tiles drawn`).toBeLessThanOrEqual(24);
    // The page still counts all of them.
    expect(document.body.textContent).toMatch(/4\d\d Spaces/);
  });
});

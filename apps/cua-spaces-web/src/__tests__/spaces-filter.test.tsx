// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Finding a Space on the Spaces page and in ⌘K, as the SwiftUI sidebar's
// search does it (the core's name/detail/id rule, plus the machine), and a
// failed start or stop said on the Space's tile (real routes, wasm core,
// the demo host).

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { useUiStore } from "@/stores/ui";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const tileNames = () => Array.from(document.querySelectorAll<HTMLElement>("[data-space-id]")).map((t) => t.getAttribute("data-space-id"));

const filterBox = () =>
  waitFor(() => {
    const el = document.querySelector<HTMLInputElement>("[data-spaces-filter]");
    if (!el) throw new Error("no filter box");
    return el;
  });

describe.skipIf(!wasmBuilt)("Spaces filter box, ⌘K keywords and tile power errors", () => {
  it("filters the tiles by name, id or machine, says when nothing matches, and Escape clears it", async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(tileNames()).toContain("relay:mac-mini/qa-windows"));
    const all = tileNames().length;
    const box = await filterBox();

    fireEvent.change(box, { target: { value: "  QA-win " } });
    await waitFor(() => expect(tileNames()).toEqual(["relay:mac-mini/qa-windows"]));

    // The machine the Space runs on.
    fireEvent.change(box, { target: { value: "linux box" } });
    await waitFor(() => expect(tileNames().length).toBeGreaterThan(0));
    expect(tileNames().every((id) => id?.startsWith("relay:linux-box/"))).toBe(true);

    fireEvent.change(box, { target: { value: "nothing-like-this" } });
    await waitFor(() => expect(tileNames()).toEqual([]));
    expect(document.body.textContent).toContain("No matches");

    fireEvent.keyDown(box, { key: "Escape" });
    await waitFor(() => expect(tileNames().length).toBe(all));
  });

  it("finds a Space in ⌘K by the machine it runs on", async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(tileNames()).toContain("relay:mac-mini/qa-windows"));
    act(() => useUiStore.getState().setPaletteOpen(true));
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>('[data-command-palette] input');
      if (!el) throw new Error("no palette");
      return el;
    });
    fireEvent.change(input, { target: { value: "mac mini" } });
    await waitFor(() => expect(document.getElementById("palette-space:open:relay:mac-mini/qa-windows")).not.toBeNull());
    act(() => useUiStore.getState().setPaletteOpen(false));
  });

  it("says on the tile why its last stop failed", async () => {
    await mountApp("/spaces", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op) => {
        if (op === "spaces.setPower") throw new Error("the machine did not answer");
        return undefined;
      },
    });
    const link = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-space-id="relay:mac-mini/qa-windows"]');
      if (!el) throw new Error("no tile");
      return el;
    });
    const tile = link.parentElement!;
    expect(tile.querySelector("[data-power-error]")).toBeNull();
    const power = await waitFor(() => {
      const b = tile.querySelector<HTMLButtonElement>("[data-tile-power]");
      if (!b) throw new Error("no power button");
      return b;
    });
    fireEvent.click(power);
    await waitFor(() => expect(tile.querySelector("[data-power-error]")?.textContent).toContain("the machine did not answer"));
  });
});

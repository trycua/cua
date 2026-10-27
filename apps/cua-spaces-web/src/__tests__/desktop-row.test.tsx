// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This Mac's desktop is a normal row of the Spaces list: "Not sharing" while its
// switch is off, "Sharing" while it is on, both with the "Desktop" tag. Hover
// shows Hide instead of power; "Show hidden" in the list's overflow brings it
// back. It never counts as a Space (the empty state, the count, "N running"),
// and Home's strip lists running Spaces first. Its page has no stream: it says
// whether the desktop is shared and holds the switch.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetHiddenDesktops } from "@/lib/hidden-desktops";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetHiddenDesktops();
  setFlagOverride("spaces_ask_first_home", true);
});
afterEach(cleanup);

/** This Mac's roster row: reachable while its desktop is shared. */
const thisMac = (shared: boolean) => ({ id: "this-mac", name: "This Mac", provider: "local", spacesdVersion: "", features: [], reachable: shared, os: "macos" });

const withDesktop = (shared: boolean, only = false) => ({
  failsCreates: false,
  listsGhost: false,
  answer: (op: string, out: unknown) => (op === "spaces.list" ? (only ? [thisMac(shared)] : [...(out as unknown[]), thisMac(shared)]) : out),
});

const row = () => document.querySelector<HTMLElement>('[data-space-id="this-mac"]');
const tile = () => row()?.parentElement ?? null;

describe.skipIf(!wasmBuilt)("This Mac's desktop row", () => {
  it("reads Not sharing with a Desktop tag, Hide in place of power, and Show hidden brings it back", async () => {
    await mountApp("/spaces", withDesktop(false));
    await waitFor(() => expect(row()).not.toBeNull());
    expect(row()!.getAttribute("data-state")).toBe("stopped");
    expect(row()!.textContent).toContain("Not sharing");
    expect(row()!.querySelector("[data-desktop-tag]")?.textContent).toBe("Desktop");
    expect(tile()!.querySelector("[data-tile-power]")).toBeNull();
    expect(tile()!.querySelector("[data-select]")).toBeNull();

    await act(async () => void fireEvent.click(tile()!.querySelector("[data-tile-hide]")!));
    await waitFor(() => expect(row()).toBeNull());
    await act(async () => void fireEvent.pointerDown(document.querySelector("[data-toolbar-menu='overflow']")!, { button: 0 }));
    await act(async () => void fireEvent.click(document.querySelector("[data-toolbar-menu='overflow']")!));
    const show = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-show-hidden]");
      expect(el).not.toBeNull();
      return el!;
    });
    await act(async () => void fireEvent.click(show));
    await waitFor(() => expect(row()).not.toBeNull());
  });

  it("never counts as a Space: the count and the running number stay the same with it listed", async () => {
    const summary = () => document.querySelector("main header p, header p")?.textContent ?? "";
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    const without = await waitFor(() => {
      expect(summary()).toMatch(/\d+ Spaces? on [1-9]/);
      return summary();
    });
    cleanup();
    await mountApp("/spaces", withDesktop(false));
    await waitFor(() => expect(row()).not.toBeNull());
    await waitFor(() => expect(summary()).toBe(without));
  });

  it("with no other Space and not sharing, the Spaces page is the normal empty state", async () => {
    await mountApp("/spaces", withDesktop(false, true));
    await waitFor(() => expect(document.querySelector("[data-empty-home]")).not.toBeNull());
    expect(row()).toBeNull();
    expect(document.querySelector("[data-space-list], [data-tile-picture]")).toBeNull();
  });

  it("with no other Space and sharing, the list holds the desktop row, running", async () => {
    await mountApp("/spaces", withDesktop(true, true));
    await waitFor(() => expect(row()).not.toBeNull());
    expect(row()!.getAttribute("data-state")).toBe("running");
    expect(row()!.textContent).toContain("Sharing");
    expect(document.querySelector("[data-empty-home]")).toBeNull();
  });

  it("its page has no stream, says You aren't sharing your desktop as a Space, and holds the switch", async () => {
    await mountApp("/spaces/this-mac", withDesktop(false));
    const page = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-this-mac-desktop]");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(page.getAttribute("data-this-mac-desktop")).toBe("off");
    expect(page.textContent).toContain("You aren't sharing your desktop as a Space");
    expect(page.textContent).toContain("from your other machines");
    expect(page.querySelector("[data-desktop-switch]")?.textContent).toContain("Let other machines control this desktop");
    expect(document.querySelector("[data-stream-surface], video, canvas")).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("Home's Your Spaces", () => {
  it("lists running Spaces first, even when the list holds them last", async () => {
    await mountApp("/home", {
      failsCreates: false,
      listsGhost: false,
      // The host lists them in the opposite order: the running ones come last.
      answer: (op, out) => (op === "spaces.list" ? [...(out as unknown[])].reverse() : out),
    });
    const states = await waitFor(() => {
      const tiles = [...document.querySelectorAll<HTMLElement>("[data-home-spaces] [data-space-id]")];
      expect(tiles.length).toBeGreaterThan(0);
      return tiles.map((t) => t.getAttribute("data-state"));
    });
    const firstOther = states.findIndex((s) => s !== "running");
    expect(states[0]).toBe("running");
    if (firstOther >= 0) expect(states.slice(firstOther).every((s) => s !== "running")).toBe(true);
    // The sidebar names the page for what it lists.
    expect(screen.getAllByText("Machines").length).toBeGreaterThan(0);
    expect(screen.queryByText("Computers")).toBeNull();
  });
});

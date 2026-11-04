// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Pin to notch (#6160) on the real routes and the wasm core: another machine's
// shared desktop has a "⋯" menu with Open, Pin to notch / Unpin from notch and
// Hide; a Space's menu has the pin above its red Delete Space; This Mac's
// desktop has no menu; a pinned row shows a small pin; the pins are the host's
// settings (`notchPinned`), kept in the order they were made; and the machine's
// page has the same menu on its desktop row.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetHiddenDesktops } from "@/lib/hidden-desktops";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetHiddenDesktops();
  setFlagOverride("spaces_ask_first_home", true);
});
afterEach(cleanup);

const DESKTOP = "relay:linux-box";
const SPACE = "local:design-review";

const thisMac = { id: "this-mac", name: "This Mac", provider: "local", spacesdVersion: "", features: [], reachable: true, os: "macos" };
const host = (extra: Partial<TestHost> = {}): TestHost => ({
  failsCreates: false,
  listsGhost: false,
  demo: { desktops: true },
  answer: (op, out) => (op === "spaces.list" ? [...(out as unknown[]), thisMac] : out),
  ...extra,
});

const tile = (id: string) => document.querySelector<HTMLElement>(`[data-space-id="${id}"]`)?.parentElement ?? null;
const more = (id: string) => document.querySelector<HTMLElement>(`[data-space-more="${id}"]`);
const labels = () => Array.from(document.querySelectorAll<HTMLElement>('[data-space-menu="more"] [role="menuitem"]')).map((e) => e.textContent);
const pinMark = (id: string) => tile(id)?.querySelector("[data-pinned-mark]") ?? null;

async function openMenu(id: string) {
  await waitFor(() => expect(more(id)).not.toBeNull());
  await act(async () => void fireEvent.click(more(id)!));
  await waitFor(() => expect(labels().length).toBeGreaterThan(0));
}

describe.skipIf(!wasmBuilt)("Pin to notch", () => {
  it("another machine's shared desktop has Open, Pin to notch and Hide, and a pin shows on the row and flips the item", async () => {
    await mountApp("/spaces", host());
    await waitFor(() => expect(tile(DESKTOP)).not.toBeNull());
    expect(tile(DESKTOP)!.querySelector("[data-desktop-tag]")?.textContent).toBe("Desktop");
    // Another machine's desktop hides from its menu; the hover button is This Mac's.
    expect(tile(DESKTOP)!.querySelector("[data-tile-hide]")).toBeNull();
    expect(pinMark(DESKTOP)).toBeNull();

    await openMenu(DESKTOP);
    expect(labels()).toEqual(["Open", "Pin to notch", "Hide"]);
    await act(async () => void fireEvent.click(document.querySelector('[data-space-menu="more"] [data-menu-item="pin"]')!));
    await waitFor(() => expect(pinMark(DESKTOP)).not.toBeNull());
    expect(pinMark(DESKTOP)!.getAttribute("aria-label")).toBe("Pinned to notch");

    await openMenu(DESKTOP);
    expect(labels()).toEqual(["Open", "Unpin from notch", "Hide"]);
    await act(async () => void fireEvent.click(document.querySelector('[data-space-menu="more"] [data-menu-item="pin"]')!));
    await waitFor(() => expect(pinMark(DESKTOP)).toBeNull());
  });

  it("Hide in the menu takes the desktop off the list, and pinning does not hide or share anything", async () => {
    await mountApp("/spaces", host());
    await waitFor(() => expect(tile(DESKTOP)).not.toBeNull());
    await openMenu(DESKTOP);
    await act(async () => void fireEvent.click(document.querySelector('[data-space-menu="more"] [data-menu-item="hide"]')!));
    await waitFor(() => expect(tile(DESKTOP)).toBeNull());
  });

  it("a Space's menu has the pin above its red Delete Space, and the pins keep the order they were made in", async () => {
    const saved: unknown[] = [];
    await mountApp("/spaces", host({ intercept: (op, args) => (op === "settings.set" ? void saved.push(args) : undefined) }));
    await waitFor(() => expect(tile(SPACE)).not.toBeNull());
    await openMenu(SPACE);
    const items = labels();
    expect(items.at(-1)).toBe("Delete Space");
    expect(items.at(-2)).toBe("Pin to notch");
    await act(async () => void fireEvent.click(document.querySelector('[data-space-menu="more"] [data-menu-item="pin"]')!));
    await waitFor(() => expect(pinMark(SPACE)).not.toBeNull());

    // A second pin goes last; the host keeps both, in order.
    await openMenu(DESKTOP);
    await act(async () => void fireEvent.click(document.querySelector('[data-space-menu="more"] [data-menu-item="pin"]')!));
    await waitFor(() => expect(pinMark(DESKTOP)).not.toBeNull());
    expect(pinMark(SPACE)).not.toBeNull();
    expect(saved).toEqual([
      { key: "notchPinned", value: [SPACE] },
      { key: "notchPinned", value: [SPACE, DESKTOP] },
    ]);
  });

  it("This Mac's desktop has no menu: it is never a notch tile", async () => {
    await mountApp("/spaces", host());
    await waitFor(() => expect(tile("this-mac")).not.toBeNull());
    expect(more("this-mac")).toBeNull();
    expect(tile("this-mac")!.querySelector("[data-tile-hide]")).not.toBeNull();
  });

  it("the machine's page has the same menu on its shared desktop row", async () => {
    await mountApp("/machines", host());
    const rowOf = () => document.querySelector<HTMLElement>(`[data-space-row="${DESKTOP}"]`);
    await waitFor(() => expect(document.querySelector('[data-machine-id="linux-box"]')).not.toBeNull());
    fireEvent.click(document.querySelector('[data-machine-id="linux-box"]')!);
    await waitFor(() => expect(rowOf()).not.toBeNull());
    await openMenu(DESKTOP);
    expect(labels()).toEqual(["Open", "Pin to notch"]);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Pin to notch (#6160) on the real routes and the wasm core, in the list of
// your computers: another computer's shared desktop has Pin to notch / Unpin
// from notch in its row's "⋯" menu; a Space's menu has the pin above its red
// Delete; This Mac's own desktop is never pinned; a pinned row shows a small
// pin; the pins are the host's settings (`notchPinned`), kept in the order
// they were made; and a computer's page has the same menu on its desktop row.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetHiddenDesktops } from "@/lib/hidden-desktops";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetHiddenDesktops();
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

const row = (key: string) => document.querySelector<HTMLElement>(`[data-tree-row="${key}"]`);
const pinMark = (key: string) => row(key)?.querySelector("[data-pinned-mark]") ?? null;
const items = () => Array.from(document.querySelectorAll<HTMLElement>('[role="menuitem"]')).map((e) => e.textContent);

async function openTreeMenu(key: string) {
  await waitFor(() => expect(document.querySelector(`[data-tree-menu="${key}"]`)).not.toBeNull());
  await act(async () => void fireEvent.click(document.querySelector(`[data-tree-menu="${key}"]`)!));
  await waitFor(() => expect(items().length).toBeGreaterThan(0));
}
const choose = async (label: string) => act(async () => void fireEvent.click(await screen.findByRole("menuitem", { name: label })));

describe.skipIf(!wasmBuilt)("Pin to notch", () => {
  it("another computer's shared desktop: Pin to notch in its row's menu, a pin on the row, and the item flips", async () => {
    await mountApp("/spaces", host());
    await waitFor(() => expect(row(DESKTOP)).not.toBeNull());
    expect(pinMark(DESKTOP)).toBeNull();
    await openTreeMenu(DESKTOP);
    expect(items()).toContain("Pin to notch");
    await choose("Pin to notch");
    await waitFor(() => expect(pinMark(DESKTOP)).not.toBeNull());
    expect(pinMark(DESKTOP)!.getAttribute("aria-label")).toBe("Pinned to notch");

    await openTreeMenu(DESKTOP);
    expect(items()).toContain("Unpin from notch");
    await choose("Unpin from notch");
    await waitFor(() => expect(pinMark(DESKTOP)).toBeNull());
  });

  it("a Space's menu has the pin above its red Delete, and the pins keep the order they were made in", async () => {
    const saved: unknown[] = [];
    await mountApp("/spaces", host({ intercept: (op, args) => (op === "settings.set" ? void saved.push(args) : undefined) }));
    await waitFor(() => expect(row(SPACE)).not.toBeNull());
    await openTreeMenu(SPACE);
    expect(items().at(-1)).toBe("Delete");
    expect(items().at(-2)).toBe("Pin to notch");
    await choose("Pin to notch");
    await waitFor(() => expect(pinMark(SPACE)).not.toBeNull());

    // A second pin goes last; the host keeps both, in order.
    await openTreeMenu(DESKTOP);
    await choose("Pin to notch");
    await waitFor(() => expect(pinMark(DESKTOP)).not.toBeNull());
    expect(pinMark(SPACE)).not.toBeNull();
    expect(saved).toEqual([
      { key: "notchPinned", value: [SPACE] },
      { key: "notchPinned", value: [SPACE, DESKTOP] },
    ]);
  });

  it("This Mac's own desktop is never a notch tile: no pin in its row's menu", async () => {
    await mountApp("/spaces", host());
    const key = await waitFor(() => {
      const r = document.querySelector<HTMLElement>('[data-tree-kind="computer"]');
      if (!r) throw new Error("no This Mac row");
      return r.getAttribute("data-tree-row")!;
    });
    await openTreeMenu(key);
    expect(items()).not.toContain("Pin to notch");
  });

  it("a computer's page has the same menu on its shared desktop row", async () => {
    await mountApp("/spaces/computer/linux-box", host());
    const rowOf = () => document.querySelector<HTMLElement>(`[data-space-row="${DESKTOP}"]`);
    await waitFor(() => expect(rowOf()).not.toBeNull());
    const more = () => document.querySelector<HTMLElement>(`[data-space-more="${DESKTOP}"]`);
    await waitFor(() => expect(more()).not.toBeNull());
    await act(async () => void fireEvent.click(more()!));
    const labels = () => Array.from(document.querySelectorAll<HTMLElement>('[data-space-menu="more"] [role="menuitem"]')).map((e) => e.textContent);
    await waitFor(() => expect(labels()).toEqual(["Open", "Pin to notch"]));
  });
});

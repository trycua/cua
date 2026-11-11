// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { DEFAULT_PREFS, SPACE_LAYOUT_STORAGE_KEY } from "@/lib/space-layout";

import { useSpaceLayoutStore } from "./space-layout";

const stored = () => JSON.parse(localStorage.getItem(SPACE_LAYOUT_STORAGE_KEY) ?? "null")?.state;

beforeEach(() => {
  localStorage.clear();
  useSpaceLayoutStore.setState({ ...DEFAULT_PREFS });
});

describe("space layout store", () => {
  it("starts vertical with every section open", () => {
    const s = useSpaceLayoutStore.getState();
    expect([s.dock, s.folded]).toEqual(["top", { people: false, children: false, stream: false, details: false }]);
  });

  it("remembers the dock and each axis's size", () => {
    const s = useSpaceLayoutStore.getState();
    s.setLayout("left", 0.4);
    s.setSplit("top", 0.6);
    expect(stored()).toMatchObject({ dock: "left", hSplit: 0.4, vSplit: 0.6 });
    s.setDock("top");
    expect(useSpaceLayoutStore.getState().hSplit).toBe(0.4);
  });

  it("remembers folded sections, and brings them back after a restart", async () => {
    useSpaceLayoutStore.getState().toggleSection("stream");
    useSpaceLayoutStore.getState().toggleSection("details");
    useSpaceLayoutStore.getState().toggleSection("details");
    expect(stored().folded).toEqual({ people: false, children: false, stream: true, details: false });
    // A restart: the state in memory is gone, the saved copy is read back.
    const saved = localStorage.getItem(SPACE_LAYOUT_STORAGE_KEY)!;
    useSpaceLayoutStore.setState({ ...DEFAULT_PREFS });
    localStorage.setItem(SPACE_LAYOUT_STORAGE_KEY, saved);
    await useSpaceLayoutStore.persist.rehydrate();
    expect(useSpaceLayoutStore.getState().folded).toEqual({ people: false, children: false, stream: true, details: false });
  });

  it("ignores a damaged saved copy", async () => {
    localStorage.setItem(SPACE_LAYOUT_STORAGE_KEY, JSON.stringify({ state: { dock: "diagonal", vSplit: "big", folded: null }, version: 0 }));
    await useSpaceLayoutStore.persist.rehydrate();
    const s = useSpaceLayoutStore.getState();
    expect([s.dock, s.vSplit, s.folded]).toEqual(["top", DEFAULT_PREFS.vSplit, DEFAULT_PREFS.folded]);
  });

  it("keeps a split saved before the layout existed, once, and never over a saved layout", async () => {
    // An upgrade: the old key is there and no layout was ever saved.
    localStorage.removeItem(SPACE_LAYOUT_STORAGE_KEY);
    localStorage.setItem("cua-spaces:call-split", "0.7");
    await useSpaceLayoutStore.persist.rehydrate();
    expect(useSpaceLayoutStore.getState().vSplit).toBe(0.7);
    expect(stored().vSplit).toBe(0.7);
    // A layout saved since wins over the old key.
    useSpaceLayoutStore.getState().setSplit("top", 0.4);
    localStorage.setItem("cua-spaces:call-split", "0.9");
    await useSpaceLayoutStore.persist.rehydrate();
    expect(useSpaceLayoutStore.getState().vSplit).toBe(0.4);
  });

  it("ignores an old split that is not a usable share", async () => {
    localStorage.removeItem(SPACE_LAYOUT_STORAGE_KEY);
    localStorage.setItem("cua-spaces:call-split", "banana");
    await useSpaceLayoutStore.persist.rehydrate();
    expect(useSpaceLayoutStore.getState().vSplit).toBe(DEFAULT_PREFS.vSplit);
  });
});

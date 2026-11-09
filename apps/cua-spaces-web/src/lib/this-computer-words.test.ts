// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The new Home and the Spaces tree name the computer they run on after its
// system: "This Mac", "This PC" on Windows, "This computer" on Linux.

import { afterEach, describe, expect, it } from "vitest";

import type { Machine, Space } from "@/bridge";

import { startErrorView, thisMacConfirmText } from "./ask-first";
import { buildSpacesTree } from "./spaces-tree";

type Host = Window & { cuaDesktop?: { invoke: () => Promise<null>; platform: string } };
const runOn = (platform: string) => {
  (window as Host).cuaDesktop = { invoke: async () => null, platform };
};
afterEach(() => {
  delete (window as Host).cuaDesktop;
});

const machine = (over: Partial<Machine>): Machine => ({
  id: "m",
  name: "M",
  os: "macos",
  via: "relay",
  connection: "relay",
  online: true,
  current: false,
  spaceIds: [],
  limits: [],
  ...over,
});
const here = machine({ id: "this-mac", name: "Ada's computer", current: true, via: "local", relayId: "here-1" });
const spaces: Space[] = [
  { id: "this-mac", name: "This Mac", os: "macos", status: "local", detail: "", lastUsedAt: 0, scene: "blank", provider: "local" } as Space,
];

const WORDS = [
  ["darwin", "This Mac", "this Mac"],
  ["win32", "This PC", "this PC"],
  ["linux", "This computer", "this computer"],
] as const;

describe.each(WORDS)("on %s", (platform, label, text) => {
  it("names this computer in the tree", () => {
    runOn(platform);
    const t = buildSpacesTree({ spaces, machines: [here], defaultSpace: null });
    expect(t.roots[0]!.name).toBe(label);
  });

  it("names it in the confirmation and the start errors", () => {
    runOn(platform);
    expect(thisMacConfirmText("Codex").title).toBe(`Work on ${text}?`);
    expect(startErrorView("this-mac: this Mac isn't shared", "Codex").title).toBe(`${label} isn't set up for tasks yet`);
    expect(startErrorView("disk full", "Codex").title).toBe(`${label} doesn't have room for a new computer`);
  });
});

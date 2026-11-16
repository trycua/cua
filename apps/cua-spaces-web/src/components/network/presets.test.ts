// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { NetworkPreset, SpaceNetwork } from "@/bridge/contracts/network";
import { networkWord } from "@/bridge/network";

import { exceeds, extrasOf, isInternetBase, networkSummary, presetWith } from "./presets";

const net = (preset: NetworkPreset, extra: Partial<SpaceNetwork> = {}): SpaceNetwork => ({ preset, allow: [], deny: [], enforced: true, ...extra });

describe("the two extras under Internet only", () => {
  it("are independent: each preset says which it has", () => {
    expect(extrasOf("internet")).toEqual({ localhost: false, lan: false });
    expect(extrasOf("localhost")).toEqual({ localhost: true, lan: false });
    expect(extrasOf("lan")).toEqual({ localhost: false, lan: true });
    expect(extrasOf("localhost+lan")).toEqual({ localhost: true, lan: true });
    expect(extrasOf("none")).toEqual({ localhost: false, lan: false });
  });

  it("combine back into a preset, both ways round", () => {
    for (const p of ["internet", "localhost", "lan", "localhost+lan"] as const) {
      expect(presetWith(extrasOf(p))).toBe(p);
      expect(isInternetBase(p)).toBe(true);
    }
    expect(isInternetBase("none")).toBe(false);
    expect(isInternetBase("custom")).toBe(false);
  });

  it("are siblings: neither is above the other", () => {
    expect(exceeds("lan", "localhost")).toBe(true);
    expect(exceeds("localhost", "lan")).toBe(true);
    expect(exceeds("localhost", "localhost")).toBe(false);
    expect(exceeds("lan", "lan")).toBe(false);
  });
});

describe("a child Space can't have more than its parent", () => {
  it("holds for the base", () => {
    expect(exceeds("internet", "none")).toBe(true);
    expect(exceeds("none", "internet")).toBe(false);
    expect(exceeds("custom", "internet")).toBe(false);
    expect(exceeds("internet", "custom")).toBe(false);
    expect(exceeds("localhost", "none")).toBe(true);
  });

  it("holds for each extra, and for both", () => {
    expect(exceeds("localhost", "internet")).toBe(true);
    expect(exceeds("lan", "internet")).toBe(true);
    expect(exceeds("localhost+lan", "internet")).toBe(true);
    expect(exceeds("localhost+lan", "localhost")).toBe(true);
    expect(exceeds("localhost+lan", "lan")).toBe(true);
    expect(exceeds("localhost+lan", "localhost+lan")).toBe(false);
    expect(exceeds("localhost", "localhost+lan")).toBe(false);
    expect(exceeds("lan", "localhost+lan")).toBe(false);
    expect(exceeds("internet", "localhost+lan")).toBe(false);
  });
});

describe("the Details summary", () => {
  it("reads naturally for each combination", () => {
    expect(networkSummary(net("internet"))).toBe("Internet only");
    expect(networkSummary(net("localhost"))).toBe("Internet and this computer's services");
    expect(networkSummary(net("lan"))).toBe("Internet and your local network");
    expect(networkSummary(net("localhost+lan"))).toBe("Internet, this computer's services and your local network");
    expect(networkSummary(net("none"))).toBe("No network");
    expect(networkSummary(net("custom"))).toBe("Custom");
  });

  it("has no dot separators", () => {
    for (const p of ["internet", "localhost", "lan", "localhost+lan", "none", "custom"] as const) expect(networkSummary(net(p))).not.toMatch(/[·•]/);
  });
});

describe("the word the host takes", () => {
  it("keeps the old words and adds the combination", () => {
    expect(networkWord(net("internet"))).toBe("internet");
    expect(networkWord(net("lan"))).toBe("lan");
    expect(networkWord(net("localhost", { localhostPorts: [5432, 3000] }))).toBe("localhost:5432,3000");
    expect(networkWord(net("localhost", { localhostPorts: [] }))).toBe("localhost:all");
    expect(networkWord(net("localhost+lan", { localhostPorts: [5432, 3000] }))).toBe("localhost+lan:5432,3000");
    expect(networkWord(net("localhost+lan", { localhostPorts: [] }))).toBe("localhost+lan:all");
  });
});

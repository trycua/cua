// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Flags: defaults, device overrides and the build's env default. The new Home and the new setup are
// not flags any more: they are always on, and a stored override of either does nothing.

import { afterEach, describe, expect, it, vi } from "vitest";

import { flagOn, setFlagOverride } from "@/bridge/flags";

afterEach(() => {
  vi.unstubAllEnvs();
  localStorage.clear();
});

describe("flags", () => {
  it("are as shipped by default: the tree is off", () => {
    expect(flagOn("spaces_tree")).toBe(false);
  });

  it("an override or the build's env turns the tree on, and an explicit off stays off", () => {
    vi.stubEnv("VITE_CUA_FLAG_SPACES_TREE", "1");
    expect(flagOn("spaces_tree")).toBe(true);
    setFlagOverride("spaces_tree", false);
    expect(flagOn("spaces_tree")).toBe(false);
    expect(localStorage.getItem("cua.flag.spaces_tree")).toBe("0");
    vi.stubEnv("VITE_CUA_FLAG_SPACES_TREE", "");
    setFlagOverride("spaces_tree", true);
    expect(flagOn("spaces_tree")).toBe(true);
  });

  it("the old flags of the new Home and the new setup are gone: a stored override is ignored", () => {
    localStorage.setItem("cua.flag.spaces_ask_first_home", "0");
    localStorage.setItem("cua.flag.spaces_new_onboarding", "0");
    expect(flagOn("spaces_tree")).toBe(false);
  });
});

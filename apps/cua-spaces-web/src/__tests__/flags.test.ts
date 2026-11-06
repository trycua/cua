// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Flags: defaults, device overrides and the build's env default.

import { afterEach, describe, expect, it, vi } from "vitest";

import { flagOn, newOnboardingOn, setFlagOverride } from "@/bridge/flags";

afterEach(() => {
  vi.unstubAllEnvs();
  localStorage.clear();
});

describe("flags", () => {
  it("are as shipped by default: the new Home and the new setup on, the tree off", () => {
    vi.stubEnv("VITE_CUA_FLAG_SPACES_ASK_FIRST_HOME", "");
    vi.stubEnv("VITE_CUA_FLAG_SPACES_NEW_ONBOARDING", "");
    setFlagOverride("spaces_ask_first_home", null);
    setFlagOverride("spaces_new_onboarding", null);
    expect(flagOn("spaces_ask_first_home")).toBe(true);
    expect(flagOn("spaces_new_onboarding")).toBe(true);
    expect(flagOn("spaces_tree")).toBe(false);
  });

  it("an explicit off stays off, over the default and the build's env; an override or the env turns one on", () => {
    vi.stubEnv("VITE_CUA_FLAG_SPACES_ASK_FIRST_HOME", "1");
    setFlagOverride("spaces_ask_first_home", false);
    expect(flagOn("spaces_ask_first_home")).toBe(false);
    // The new setup needs the new Home: off with it.
    setFlagOverride("spaces_new_onboarding", true);
    expect(newOnboardingOn()).toBe(false);
    vi.stubEnv("VITE_CUA_FLAG_SPACES_ASK_FIRST_HOME", "");
    setFlagOverride("spaces_ask_first_home", true);
    expect(flagOn("spaces_ask_first_home")).toBe(true);
    expect(newOnboardingOn()).toBe(true);
    // Off for this device and still off after "a restart" (the choice is what is stored).
    setFlagOverride("spaces_new_onboarding", false);
    expect(newOnboardingOn()).toBe(false);
    expect(localStorage.getItem("cua.flag.spaces_new_onboarding")).toBe("0");
    // The tree is still off until turned on.
    expect(flagOn("spaces_tree")).toBe(false);
    setFlagOverride("spaces_tree", true);
    expect(flagOn("spaces_tree")).toBe(true);
  });
});

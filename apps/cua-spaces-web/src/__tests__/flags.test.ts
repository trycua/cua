// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Flags: defaults, device overrides and the build's env default.

import { afterEach, describe, expect, it, vi } from "vitest";

import { flagOn, setFlagOverride } from "@/bridge/flags";

afterEach(() => {
  vi.unstubAllEnvs();
  localStorage.clear();
});

describe("flags", () => {
  it("are off by default; an override or the build's env turns one on", () => {
    vi.stubEnv("VITE_CUA_FLAG_SPACES_ASK_FIRST_HOME", "");
    setFlagOverride("spaces_ask_first_home", null);
    expect(flagOn("spaces_ask_first_home")).toBe(false);
    vi.stubEnv("VITE_CUA_FLAG_SPACES_ASK_FIRST_HOME", "1");
    expect(flagOn("spaces_ask_first_home")).toBe(true);
    // This device's explicit choice wins over the env, both ways.
    setFlagOverride("spaces_ask_first_home", false);
    expect(flagOn("spaces_ask_first_home")).toBe(false);
    vi.stubEnv("VITE_CUA_FLAG_SPACES_ASK_FIRST_HOME", "");
    setFlagOverride("spaces_ask_first_home", true);
    expect(flagOn("spaces_ask_first_home")).toBe(true);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Flags: defaults, overrides, and the retired `spaces_redesign` (the classic design is gone, so a
// leftover override of it, such as a stale "0" from a test, is cleared and never read).

import { afterEach, describe, expect, it, vi } from "vitest";

import { clearRetiredFlagOverrides, flagOn, setFlagOverride } from "@/bridge/flags";

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

  it("clears a stale spaces_redesign override on start, and leaves the other flags alone", () => {
    localStorage.setItem("cua.flag.spaces_redesign", "0");
    localStorage.setItem("cua.flag.spaces_ask_first_home", "1");
    clearRetiredFlagOverrides();
    expect(localStorage.getItem("cua.flag.spaces_redesign")).toBeNull();
    expect(localStorage.getItem("cua.flag.spaces_ask_first_home")).toBe("1");
  });
});

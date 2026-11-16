// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { RETIRED_FLAGS, clearRetiredFlagOverrides } from "@/bridge/flags";

describe("retired flags", () => {
  it("clears a device's leftover override of every retired flag, whatever it was set to", () => {
    for (const name of RETIRED_FLAGS) localStorage.setItem(`cua.flag.${name}`, "0");
    localStorage.setItem("cua.flag.spaces_ask_first_home", "1");
    localStorage.setItem("cua.other", "kept");
    clearRetiredFlagOverrides();
    for (const name of RETIRED_FLAGS) expect(localStorage.getItem(`cua.flag.${name}`)).toBeNull();
    expect(localStorage.getItem("cua.other")).toBe("kept");
  });

  it("includes the new Home and the new setup, now the defaults", () => {
    expect(RETIRED_FLAGS).toEqual(expect.arrayContaining(["spaces_ask_first_home", "spaces_new_onboarding", "spaces_tree"]));
  });
});

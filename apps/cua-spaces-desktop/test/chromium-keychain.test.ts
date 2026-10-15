// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { applyChromiumKeychainSwitches, chromiumKeychainSwitches } from "../src/chromium-keychain";

describe("chromiumKeychainSwitches", () => {
  it("keeps Chromium out of the macOS keychain", () => {
    expect(chromiumKeychainSwitches("darwin")).toEqual(["use-mock-keychain"]);
  });

  it("changes nothing elsewhere", () => {
    expect(chromiumKeychainSwitches("win32")).toEqual([]);
    expect(chromiumKeychainSwitches("linux")).toEqual([]);
  });

  it("appends the switches to the command line", () => {
    const added: string[] = [];
    applyChromiumKeychainSwitches({ appendSwitch: (name) => added.push(name) }, "darwin");
    expect(added).toEqual(["use-mock-keychain"]);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { hideDesktop, readHidden, resetHiddenDesktops, showDesktop, showHiddenDesktops } from "./hidden-desktops";

beforeEach(() => {
  localStorage.clear();
  resetHiddenDesktops();
});

describe("hidden desktops", () => {
  it("are kept per account on this device, and signed out has its own", () => {
    hideDesktop("ada@example.com", "relay:studio");
    hideDesktop(null, "this-mac");
    expect([...readHidden("ada@example.com")]).toEqual(["relay:studio"]);
    expect([...readHidden("bob@example.com")]).toEqual([]);
    expect([...readHidden(null)]).toEqual(["this-mac"]);
    // They outlive the window: read again from storage.
    resetHiddenDesktops();
    expect([...readHidden("ada@example.com")]).toEqual(["relay:studio"]);
  });

  it("come back one at a time or all at once", () => {
    hideDesktop("ada@example.com", "relay:studio");
    hideDesktop("ada@example.com", "this-mac");
    showDesktop("ada@example.com", "this-mac");
    expect([...readHidden("ada@example.com")]).toEqual(["relay:studio"]);
    showHiddenDesktops("ada@example.com");
    expect(readHidden("ada@example.com").size).toBe(0);
    expect(localStorage.getItem("cua.hiddenDesktops:ada@example.com")).toBeNull();
  });

  it("hidden before they were kept per account move to the first account that looks, so they can be shown again", () => {
    localStorage.setItem("cua.hiddenDesktops", JSON.stringify(["relay:studio"]));
    expect([...readHidden("ada@example.com")]).toEqual(["relay:studio"]);
    expect(localStorage.getItem("cua.hiddenDesktops")).toBeNull();
    expect([...readHidden("bob@example.com")]).toEqual([]);
  });
});

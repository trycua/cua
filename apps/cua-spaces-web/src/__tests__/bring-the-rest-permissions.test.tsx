// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Home's apps card: each permission live, and On once both are (CUA-3551). Setup connects Cua Driver itself (CUA-4067). */

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AppPermissions, DriverPermission } from "@/bridge/ops/driver";
import { AppsOnThisMacCard } from "@/components/ask-first/bring-the-rest";
import { homeAppsRowLine, homeAppsRowShown } from "@/lib/app-permissions";

const SCREEN = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
const AX = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
type State = NonNullable<DriverPermission["state"]>;
const answer = (screenState: State, axState: State): AppPermissions => ({
  appName: "Cua Spaces Local",
  permissions: [
    { id: "screen", granted: screenState === "on", state: screenState, settingsUrl: SCREEN },
    { id: "accessibility", granted: axState === "on", state: axState, settingsUrl: AX },
  ],
});

describe("Home's apps row on its own (CUA-3551)", () => {
  afterEach(cleanup);

  it("shows on a Mac whenever a permission is missing, and stays once shown so the switches turn On in place", () => {
    const base = { os: "macos", available: true, allGranted: false, shownBefore: false };
    expect(homeAppsRowShown(base)).toBe(true);
    expect(homeAppsRowShown({ ...base, allGranted: true })).toBe(false);
    expect(homeAppsRowShown({ ...base, allGranted: true, shownBefore: true })).toBe(true);
    expect(homeAppsRowShown({ ...base, available: false })).toBe(false);
    expect(homeAppsRowShown({ ...base, os: "windows" })).toBe(false);
  });

  it("says how many switches are still off", () => {
    expect(homeAppsRowLine([{ granted: false }, { granted: false }])).toBe("Two switches in System Settings are still off.");
    expect(homeAppsRowLine([{ granted: true }, { granted: false }])).toBe("One switch in System Settings is still off.");
    expect(homeAppsRowLine([{ granted: true }, { granted: true }])).toContain("Both are on.");
  });

  it("draws each switch live, with Allow only where it is off, and On once both are", () => {
    const allow = vi.fn(async () => {});
    const { rerender } = render(<AppsOnThisMacCard os="macos" perms={{ data: answer("on", "off"), allow, allGranted: false }} />);
    expect(document.querySelector("#home-apps-heading")!.textContent).toBe("Let agents use apps on this Mac");
    expect(document.querySelector("[data-app-permission=accessibility] [data-permission-status]")!.getAttribute("data-permission-status")).toBe("off");
    expect(document.querySelector("[data-app-permission=accessibility] button")!.textContent).toBe("Allow");
    expect(document.querySelector("[data-app-permission=screen] button")).toBeNull();
    expect(document.querySelector("[data-home-apps-done]")).toBeNull();
    rerender(<AppsOnThisMacCard os="macos" perms={{ data: answer("on", "off"), allow, allGranted: false, waiting: true }} />);
    expect(document.querySelector("[data-app-permission=accessibility] [data-permission-status]")!.getAttribute("data-permission-status")).toBe("checking");
    rerender(<AppsOnThisMacCard os="macos" perms={{ data: answer("on", "on"), allow, allGranted: true }} />);
    expect(document.querySelector("[data-home-apps-done]")!.textContent).toContain("On");
    expect(document.querySelector("[data-home-apps]")!.textContent).toContain("Both are on.");
  });
});

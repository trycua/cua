// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Home's "Now bring the rest in": the apps row shows each permission live, and On once both are (CUA-3551). */

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AppPermissions, DriverPermission } from "@/bridge/ops/driver";
import { AppsPanel, BringTheRestCard } from "@/components/ask-first/bring-the-rest";
import { bringRestView } from "@/lib/first-run-new";

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

describe("Home's 'Now bring the rest in'", () => {
  afterEach(cleanup);

  it("marks the apps row On once both are on, and its panel shows each switch live", () => {
    const view = bringRestView({ os: "macos" });
    const data = answer("on", "off");
    const { rerender } = render(<BringTheRestCard view={view} open="apps" onRow={() => {}} onDismiss={() => {}} apps={<AppsPanel perms={{ data, allow: vi.fn(async () => {}), waiting: true }} />} done={{ apps: false }} />);
    expect(document.querySelector("[data-bring-rest-done=apps]")).toBeNull();
    expect(document.querySelector("[data-app-permission=screen] [data-permission-status]")!.getAttribute("data-permission-status")).toBe("on");
    expect(document.querySelector("[data-app-permission=accessibility] [data-permission-status]")!.getAttribute("data-permission-status")).toBe("checking");
    const both = answer("on", "on");
    rerender(<BringTheRestCard view={view} open="apps" onRow={() => {}} onDismiss={() => {}} apps={<AppsPanel perms={{ data: both, allow: vi.fn(async () => {}) }} />} done={{ apps: true }} />);
    expect(document.querySelector("[data-bring-rest-done=apps]")!.textContent).toContain("On");
    expect(document.querySelector("[data-bring-rest-apps]")!.textContent).toContain("Both are on.");
  });
});

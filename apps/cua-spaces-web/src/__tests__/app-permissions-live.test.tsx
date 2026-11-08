// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The permission rows' live status (CUA-3551): where each switch stands, a check and "On" once it is granted, and
 * "Waiting for the switch" after Allow, also after the app reopens for Screen Recording.
 */

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { APP_PERMISSIONS_ASKED_KEY, readAskedAt, useAppPermissions } from "@/bridge/app-permissions";
import { BridgeContext, type BridgeContextValue } from "@/bridge/BridgeProvider";
import type { AppPermissions, DriverPermission } from "@/bridge/ops/driver";
import { AppPermissionList } from "@/components/app-permissions";
import { allowHint, permissionState, permissionStatus } from "@/lib/app-permissions";

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

describe("where a permission stands", () => {
  it("reads on, off (in the list), absent (not in the list) and unknown, from old hosts too", () => {
    expect(permissionState({ granted: true, state: "off" })).toBe("on");
    expect(permissionState({ granted: false, state: "off" })).toBe("off");
    expect(permissionState({ granted: false, state: "absent" })).toBe("absent");
    expect(permissionState({ granted: false })).toBe("unknown");
    expect(permissionState({ granted: false, state: "on" })).toBe("unknown");
  });

  it("says On once granted, that it is waiting after Allow, else what System Settings shows", () => {
    expect(permissionStatus({ granted: true, state: "on" }, true)).toEqual({ label: "On", done: true, checking: false });
    expect(permissionStatus({ granted: false, state: "off" }, true)).toEqual({ label: "Waiting for the switch", done: false, checking: true });
    expect(permissionStatus({ granted: false, state: "off" }, false)?.label).toBe("Off");
    expect(permissionStatus({ granted: false, state: "absent" }, false)?.label).toBe("Not in the list yet");
    expect(permissionStatus({ granted: false }, false)).toBeNull();
    expect(allowHint("Cua Spaces Local", true)).toBe("Both are on. Agents can use apps on this Mac.");
  });
});

describe("the rows, live", () => {
  let reads: AppPermissions[];
  let calls: { op: string; args: unknown }[];
  const wrapper = ({ children }: { children?: ReactNode }) => {
    const adapter = {
      call: vi.fn(async (op: string, args: unknown) => {
        calls.push({ op, args });
        if (op === "driver.permissions") return reads.length > 1 ? reads.shift() : reads[0];
        return null;
      }),
    };
    const value = { mode: "electron", core: {}, adapter, store: null, ready: Promise.resolve(null) } as unknown as BridgeContextValue;
    return <BridgeContext.Provider value={value}>{children}</BridgeContext.Provider>;
  };
  function Rows() {
    const perms = useAppPermissions();
    return <AppPermissionList perms={perms} variant="hero" />;
  }
  const status = (id: string) => document.querySelector(`[data-app-permission=${id}] [data-permission-status]`)?.getAttribute("data-permission-status") ?? null;

  beforeEach(() => {
    calls = [];
    window.localStorage.clear();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    window.localStorage.clear();
  });

  it("shows each switch as System Settings has it, and Allow only where it is off", async () => {
    reads = [answer("absent", "off")];
    render(<Rows />, { wrapper });
    await waitFor(() => expect(status("screen")).toBe("absent"));
    expect(status("accessibility")).toBe("off");
    expect(document.querySelector("[data-app-permission=screen]")!.textContent).toContain("Not in the list yet");
    expect(screen.getAllByRole("button", { name: "Allow" })).toHaveLength(2);
  });

  it("says it is waiting after Allow, then turns each row to On as it is granted", async () => {
    reads = [answer("off", "absent")];
    render(<Rows />, { wrapper });
    await waitFor(() => expect(status("screen")).toBe("off"));
    fireEvent.click(document.querySelector("[data-app-permission=screen] button")!);
    await waitFor(() => expect(calls.some((c) => c.op === "host.openSettings")).toBe(true));
    expect(calls.find((c) => c.op === "host.openSettings")!.args).toEqual({ url: SCREEN });
    expect(readAskedAt()).not.toBeNull();
    await waitFor(() => expect(status("screen")).toBe("checking"));
    expect(document.querySelector("[data-app-permission=screen]")!.textContent).toContain("Waiting for the switch");
    // The person turns both on in System Settings: the next reads say so, with no click here.
    reads = [answer("on", "on")];
    await waitFor(() => expect(status("screen")).toBe("on"), { timeout: 3000 });
    expect(status("accessibility")).toBe("on");
    expect(document.querySelector("[data-app-permission=screen]")!.textContent).toContain("On");
    expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
    expect(document.body.textContent).toContain("Both are on.");
    // Nothing left to wait for after the next launch either.
    await waitFor(() => expect(window.localStorage.getItem(APP_PERMISSIONS_ASKED_KEY)).toBeNull());
  });

  it("after a relaunch (Quit & Reopen), still says it is checking and shows the grant", async () => {
    // Allow was pressed a minute ago, before the app reopened.
    window.localStorage.setItem(APP_PERMISSIONS_ASKED_KEY, String(Date.now() - 60_000));
    reads = [answer("off", "absent")];
    render(<Rows />, { wrapper });
    await waitFor(() => expect(status("screen")).toBe("checking"));
    expect(status("accessibility")).toBe("checking");
    reads = [answer("on", "absent")];
    await waitFor(() => expect(status("screen")).toBe("on"), { timeout: 3000 });
    expect(status("accessibility")).toBe("checking");
  });

  it("reads again when the window comes back to the front", async () => {
    reads = [answer("off", "off")];
    render(<Rows />, { wrapper });
    await waitFor(() => expect(status("screen")).toBe("off"));
    reads = [answer("on", "on")];
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(status("screen")).toBe("on"));
  });

  it("forgets an Allow older than 15 minutes", () => {
    window.localStorage.setItem(APP_PERMISSIONS_ASKED_KEY, String(Date.now() - 16 * 60_000));
    expect(readAskedAt()).toBeNull();
  });
});

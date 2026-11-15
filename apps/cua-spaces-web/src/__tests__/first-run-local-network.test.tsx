// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run asks for Local Network on a Mac that runs macOS Spaces (CUA-3854): the new first run had dropped the
 * row, so the issue "Cua can't reach this Mac's macOS Spaces" showed up later on the computer's page. macOS asks only
 * when the app first connects, so nothing is checked until Allow, right after the step says why; then the answer shows
 * live, and once macOS was told no the button opens the Local Network settings.
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { BridgeContext, type BridgeContextValue } from "@/bridge/BridgeProvider";
import { LOCAL_NETWORK_ASKED_KEY, useLocalNetwork, type LocalNetworkState } from "@/bridge/local-network";
import type { AppPermissions } from "@/bridge/ops/driver";
import { LocalNetworkRow } from "@/components/app-permissions";
import { PermissionsScreen } from "@/components/first-run-new/first-run-new";
import { localNetworkStatus, permissionsStepView } from "@/lib/app-permissions";

const LN = "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";
const SCREEN = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
const AX = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
const both = (on: boolean, appName = "Cua Spaces Local"): AppPermissions => ({
  appName,
  permissions: [
    { id: "screen", granted: on, settingsUrl: SCREEN },
    { id: "accessibility", granted: on, settingsUrl: AX },
  ],
});
const net = (over: Partial<LocalNetworkState> = {}): LocalNetworkState => ({
  loading: false,
  available: true,
  granted: false,
  asked: false,
  waiting: false,
  denied: false,
  allow: vi.fn(async () => {}),
  ...over,
});
const text = () => document.body.textContent ?? "";

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

describe("the permissions step on a Mac that runs macOS Spaces", () => {
  const draw = (d: AppPermissions, n: LocalNetworkState) => {
    const ln = !n.available ? "none" : n.granted ? "on" : n.denied ? "denied" : "off";
    const onContinue = vi.fn();
    render(
      <PermissionsScreen
        view={permissionsStepView({ appName: d.appName, allGranted: d.permissions.every((p) => p.granted), localNetwork: ln })}
        perms={{ data: d, allow: vi.fn(async () => {}) }}
        net={n}
        appName={d.appName}
        onContinue={onContinue}
      />,
    );
    return { onContinue };
  };

  it("adds a third row, says why before macOS asks, and Allow makes macOS ask", () => {
    const n = net();
    draw(both(false), n);
    expect(screen.getByRole("heading").textContent).toBe("Let Cua Spaces Local use this Mac");
    expect(text()).toContain("and macOS Spaces can run here");
    const rows = [...document.querySelectorAll<HTMLElement>("[data-app-permission]")].map((r) => r.dataset.appPermission);
    expect(rows).toEqual(["screen", "accessibility", "local-network"]);
    const row = document.querySelector<HTMLElement>("[data-app-permission=local-network]")!;
    expect(row.textContent).toContain("Run macOS Spaces on this Mac");
    expect(row.textContent).toContain("Local Network, given to Cua Spaces Local");
    // Nothing checked yet, so no status: just Allow.
    expect(row.querySelector("[data-permission-status]")).toBeNull();
    expect(text()).toContain(
      "macOS Spaces run on a small network inside this Mac. After Allow, macOS asks if Cua Spaces Local can find devices on your local network. Click Allow there too.",
    );
    fireEvent.click(row.querySelector("button")!);
    expect(n.allow).toHaveBeenCalled();
  });

  it("waits for macOS's answer, then shows On; Continue once all three are on", () => {
    draw(both(true), net({ asked: true, waiting: true }));
    const row = () => document.querySelector<HTMLElement>("[data-app-permission=local-network]")!;
    expect(row().querySelector("[data-permission-status]")!.getAttribute("data-permission-status")).toBe("checking");
    expect(row().textContent).toContain("Waiting for your answer");
    expect(row().querySelector("button")).toBeNull();
    // Until it is on, the button says the run goes on without it, and what is still off.
    expect(screen.getByRole("button", { name: "Continue without it" })).toBeTruthy();
    expect(document.querySelector("[data-new-run-still-off]")!.textContent).toBe("Still off: Run macOS Spaces on this Mac");
    cleanup();
    draw(both(true), net({ asked: true, granted: true }));
    expect(row().querySelector("[data-permission-status]")!.getAttribute("data-permission-status")).toBe("on");
    expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy();
    expect(text()).toContain("All on. Agents can use apps on this Mac, and macOS Spaces can run here.");
  });

  it("said no: Off, and the button opens the Local Network settings", () => {
    const n = net({ asked: true, denied: true });
    draw(both(true), n);
    const row = document.querySelector<HTMLElement>("[data-app-permission=local-network]")!;
    expect(row.querySelector("[data-permission-status]")!.textContent).toBe("Off");
    expect(row.querySelector("button")!.textContent).toBe("Open Settings");
    expect(text()).toContain("turn on Cua Spaces Local in System Settings, under Privacy & Security, Local Network.");
    fireEvent.click(row.querySelector("button")!);
    expect(n.allow).toHaveBeenCalled();
    // Never blocks the run.
    expect(screen.getByRole("button", { name: "Continue without it" })).toBeTruthy();
  });

  // Device test 10 Nov: on a short window the Local Network row was cut off under the fade, so the button stayed
  // "Later" with both visible switches on and nothing said why.
  it("names the row still off and shows it on request, wherever it is on the page", () => {
    draw(both(true), net());
    const row = document.querySelector<HTMLElement>("[data-app-permission=local-network]")!;
    const scrolled = vi.fn();
    row.scrollIntoView = scrolled;
    const stillOff = document.querySelector<HTMLButtonElement>("[data-new-run-still-off]")!;
    expect(stillOff.textContent).toBe("Still off: Run macOS Spaces on this Mac");
    fireEvent.click(stillOff);
    expect(scrolled).toHaveBeenCalled();
    expect(document.activeElement).toBe(row.querySelector("button"));
  });

  it("a Mac that doesn't run macOS Spaces keeps the two rows and the old words", () => {
    draw(both(false, "Cua Spaces"), net({ available: false }));
    expect(screen.getByRole("heading").textContent).toBe("Let Cua Spaces see and use apps");
    expect(document.querySelector("[data-app-permission=local-network]")).toBeNull();
  });

  it("the row's status words", () => {
    expect(localNetworkStatus({ granted: false, waiting: false, denied: false })).toBeNull();
    expect(localNetworkStatus({ granted: false, waiting: true, denied: false })?.label).toBe("Waiting for your answer");
    expect(localNetworkStatus({ granted: false, waiting: false, denied: true })?.label).toBe("Off");
    expect(localNetworkStatus({ granted: true, waiting: false, denied: false })?.label).toBe("On");
  });
});

describe("useLocalNetwork", () => {
  let granted: boolean;
  let listed: boolean;
  let calls: { op: string; args: Record<string, unknown> }[];
  const wrapper = ({ children }: { children?: ReactNode }) => {
    const adapter = {
      call: vi.fn(async (op: string, args: Record<string, unknown>) => {
        calls.push({ op, args });
        if (op === "host.status") {
          const permissions = listed && args.localNetwork ? [{ id: "local-network", label: "Local Network", settingsUrl: LN, granted }] : [];
          return { configured: true, permissions };
        }
        if (op === "host.openSettings") granted = true;
        return null;
      }),
    };
    const value = { mode: "electron", core: {}, adapter, store: null, ready: Promise.resolve(null) } as unknown as BridgeContextValue;
    return <BridgeContext.Provider value={value}>{children}</BridgeContext.Provider>;
  };
  function Row() {
    const n = useLocalNetwork(true);
    return n.available ? <LocalNetworkRow net={n} appName="Cua Spaces Local" /> : <p data-none="" />;
  }
  beforeEach(() => {
    granted = false;
    listed = true;
    calls = [];
    window.localStorage.clear();
  });

  it("checks nothing before Allow, then asks through the Local Network pane and shows the answer", async () => {
    render(<Row />, { wrapper });
    await waitFor(() => expect(document.querySelector("[data-app-permission=local-network]")).not.toBeNull());
    const reads = calls.filter((c) => c.op === "host.status");
    expect(reads.length).toBeGreaterThan(0);
    // Listed, never checked: a check would make macOS ask before the step explained why.
    expect(reads.every((c) => c.args.localNetwork === true && c.args.localNetworkCheck === false)).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(calls.some((c) => c.op === "host.openSettings" && c.args.url === LN)).toBe(true));
    expect(Number(window.localStorage.getItem(LOCAL_NETWORK_ASKED_KEY))).toBeGreaterThan(0);
    await waitFor(() =>
      expect(document.querySelector("[data-app-permission=local-network] [data-permission-status]")?.getAttribute("data-permission-status")).toBe("on"),
    );
    expect(calls.filter((c) => c.op === "host.status").at(-1)!.args.localNetworkCheck).toBe(true);
  });

  it("asked before (an earlier launch): checks at once", async () => {
    window.localStorage.setItem(LOCAL_NETWORK_ASKED_KEY, "1");
    granted = true;
    render(<Row />, { wrapper });
    await waitFor(() => expect(document.querySelector("[data-app-permission=local-network]")?.hasAttribute("data-granted")).toBe(true));
    expect(calls.filter((c) => c.op === "host.status").every((c) => c.args.localNetworkCheck === true)).toBe(true);
  });

  it("a computer that doesn't run macOS Spaces has no row", async () => {
    listed = false;
    render(<Row />, { wrapper });
    await waitFor(() => expect(calls.some((c) => c.op === "host.status")).toBe(true));
    expect(document.querySelector("[data-none]")).not.toBeNull();
  });
});

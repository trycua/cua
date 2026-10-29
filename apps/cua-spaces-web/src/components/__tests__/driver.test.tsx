// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider, pointerChange, pointerSettings, setFlagOverride } from "@/bridge";
import { UnsupportedOperationError, type DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import { DriverPage } from "@/components/driver/driver-page";
import { TopBarDriver } from "@/components/driver/top-bar-driver";
import { useNavItems } from "@/components/shell/nav";
import { ToastProvider } from "@/components/ui/toast";

beforeEach(() => setFlagOverride("spaces_embedded_driver", true));
afterEach(() => {
  cleanup();
  setFlagOverride("spaces_embedded_driver", null);
});

function mount(adapter: DataAdapter, ui: () => ReactNode) {
  const router = createRouter({ routeTree: createRootRoute({ component: () => <>{ui()}</> }), history: createMemoryHistory({ initialEntries: ["/"] }) });
  return render(
    <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
      <ToastProvider>
        <RouterProvider router={router} />
      </ToastProvider>
    </BridgeProvider>,
  );
}

/** A host with no driver operations (the native hosts today). */
function withoutDriver(): DataAdapter {
  const base = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
  return Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: unknown) =>
      op.startsWith("driver.") ? Promise.reject(new UnsupportedOperationError("webkit", op)) : (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args)) as DataAdapter["call"],
  });
}

function NavLabels() {
  return <div data-nav="">{useNavItems().map((n) => n.label).join(",")}</div>;
}

describe("pointer settings from the driver's get_config", () => {
  it("hide every setting the driver doesn't report", () => {
    expect(pointerSettings({ agent_cursor: { enabled: true }, cursor: { badges: "minimal", motion: { style: "signature_arc" } } })).toEqual({
      show: true,
      style: "signature_arc",
      float: null,
      marks: null,
      rotate: null,
      nameTag: "minimal",
    });
    expect(pointerSettings({ cursor: { float: "on", marks: "off", rotate: false } })).toMatchObject({ show: null, float: true, marks: false, rotate: false });
    expect(pointerSettings(null)).toEqual({ show: null, style: null, float: null, marks: null, rotate: null, nameTag: null });
  });

  it("map each change to its set_config key", () => {
    expect(pointerChange("float", false)).toEqual({ "cursor.float": "off" });
    expect(pointerChange("marks", true)).toEqual({ "cursor.marks": "on" });
    expect(pointerChange("style", "magnetic")).toEqual({ "cursor.motion.style": "magnetic" });
    expect(pointerChange("nameTag", "off")).toEqual({ "cursor.badges": "off" });
    expect(pointerChange("show", false)).toEqual({ "agent_cursor.enabled": false });
  });
});

describe("Cua Driver in the app", () => {
  it("shows the sidebar item under Machines only with the flag on and a host that runs the driver", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    mount(demo, () => <NavLabels />);
    await waitFor(() => expect(document.querySelector("[data-nav]")?.textContent).toContain("Machines,Cua Driver,Agents"));
    cleanup();

    mount(withoutDriver(), () => <NavLabels />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(document.querySelector("[data-nav]")?.textContent).not.toContain("Cua Driver");
    cleanup();

    setFlagOverride("spaces_embedded_driver", false);
    mount(createDemoAdapter({ latencyMs: 0, stepMs: 0 }), () => <NavLabels />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(document.querySelector("[data-nav]")?.textContent).not.toContain("Cua Driver");
  });

  it("draws the page: state, version with Update, Test now, pointer rows and coding apps", async () => {
    mount(createDemoAdapter({ latencyMs: 0, stepMs: 0 }), () => <DriverPage machine={null} />);
    expect(await screen.findByText("Working on this Mac.")).toBeTruthy();
    expect(screen.getByText("Lets agents see and use the apps on this computer.")).toBeTruthy();
    expect(document.querySelector("[data-driver-version]")?.textContent).toBe("Version 0.38.0. Version 0.38.1 is ready: faster clicks in Chrome.");
    expect(screen.getByRole("button", { name: "Update to 0.38.1" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Test now" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Pause agents" })).toBeTruthy();
    await waitFor(() => expect(screen.getByLabelText("Float while it thinks")).toBeTruthy());
    expect(screen.getByLabelText("Tilt while moving")).toBeTruthy();
    expect(document.querySelector("[data-driver-app=claude-code]")?.textContent).toContain("Uses Cua Driver");
    expect(document.querySelector("[data-driver-app=t3-code]")?.textContent).toContain("Brings its own Cua Driver");
    expect(document.querySelector("[data-driver-computers]")?.textContent).toContain("Mac mini");
  });

  it("hides Update when the backend reports none, and a pointer setting the driver lacks", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    demo.state.driver.update = null;
    delete (demo.state.driver.config.cursor as Record<string, unknown>).rotate;
    mount(demo, () => <DriverPage machine={null} />);
    expect(await screen.findByText("Working on this Mac.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Update/ })).toBeNull();
    await waitFor(() => expect(screen.getByLabelText("Float while it thinks")).toBeTruthy());
    expect(screen.queryByLabelText("Tilt while moving")).toBeNull();
  });

  it("changes a pointer setting through set_config", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    mount(demo, () => <DriverPage machine={null} />);
    const float = await screen.findByLabelText("Float while it thinks");
    fireEvent.click(float);
    await waitFor(() => expect((demo.state.driver.config.cursor as Record<string, unknown>).float).toBe(false));
  });

  it("puts 'Agents can use this Mac' in the top bar, opening a panel with Pause and Update", async () => {
    mount(createDemoAdapter({ latencyMs: 0, stepMs: 0 }), () => <TopBarDriver />);
    const line = await screen.findByRole("button", { name: "Agents can use this Mac" });
    fireEvent.click(line);
    const panel = await waitFor(() => {
      const p = document.querySelector("[data-driver-panel]");
      expect(p).toBeTruthy();
      return p!;
    });
    expect(panel.textContent).toContain("Claude Code is using Xcode");
    expect(panel.textContent).toContain("Version 0.38.1 is ready");
    expect(panel.textContent).toContain("Cua Driver settings");
  });

  it("draws nothing in the top bar where the host has no driver", async () => {
    mount(withoutDriver(), () => <TopBarDriver />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(document.querySelector("[data-top-bar-driver]")).toBeNull();
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider, pointerChange, pointerSettings } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import { DriverPage } from "@/components/driver/driver-page";
import { useNavItems } from "@/components/shell/nav";
import { ToastProvider } from "@/components/ui/toast";

afterEach(() => cleanup());

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

function NavLabels() {
  return (
    <div data-nav="">
      {useNavItems()
        .map((n) => n.label)
        .join(",")}
    </div>
  );
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
  it("is no nav item of its own: it is one of This Mac's settings, a line under Computers (CUA-3551)", async () => {
    mount(createDemoAdapter({ latencyMs: 0, stepMs: 0 }), () => <NavLabels />);
    await waitFor(() => expect(document.querySelector("[data-nav]")?.textContent).toMatch(/^Home,Computers,Agents/));
    expect(document.querySelector("[data-nav]")?.textContent).not.toMatch(/Cua Driver|Machines/);
  });

  it("draws the page: state, version, Repair, Test now, the switch, pointer rows, agents and permissions", async () => {
    mount(createDemoAdapter({ latencyMs: 0, stepMs: 0 }), () => <DriverPage machine={null} />);
    expect(await screen.findByText("Working on this Mac.")).toBeTruthy();
    expect(screen.getByText("Lets agents see and use the apps on this Mac.")).toBeTruthy();
    expect(document.querySelector("[data-driver-version]")?.textContent).toBe("Version 0.38.0.");
    expect(screen.queryByRole("button", { name: /Update/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Repair" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Test now" })).toBeTruthy();
    expect((screen.getByLabelText("Let agents use apps on this Mac") as HTMLElement).getAttribute("aria-checked")).toBe("true");
    await waitFor(() => expect(screen.getByLabelText("Float while it thinks")).toBeTruthy());
    expect(document.querySelector("[data-driver-app=claude-code]")?.textContent).toContain("Uses Cua Driver in Cua Spaces.");
    expect(document.querySelector("[data-driver-app=codex]")?.textContent).toContain("Not connected. Set up connects it.");
    expect(document.querySelector("[data-driver-app=t3-code]")?.textContent).toContain("Brings its own Cua Driver");
    await waitFor(() => expect(document.querySelector("[data-app-permission=screen]")?.textContent).toContain("On"));
    expect(document.querySelector("[data-driver-computers]")).toBeNull();
  });

  it("hides a pointer setting the driver lacks", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    delete (demo.state.driver.config.cursor as Record<string, unknown>).rotate;
    mount(demo, () => <DriverPage machine={null} />);
    expect(await screen.findByText("Working on this Mac.")).toBeTruthy();
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

  it("turns agents off and on with the switch", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    mount(demo, () => <DriverPage machine={null} />);
    const allow = await screen.findByLabelText("Let agents use apps on this Mac");
    fireEvent.click(allow);
    await waitFor(() => expect(demo.state.driver.paused).toBe(true));
    expect(await screen.findByText("Off. Agents can't use apps on this Mac.")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("Let agents use apps on this Mac"));
    await waitFor(() => expect(demo.state.driver.paused).toBe(false));
  });

  it("Set up connects every coding agent and tests the driver", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    for (const a of demo.state.driver.apps) a.on = false;
    mount(demo, () => <DriverPage machine={null} />);
    fireEvent.click(await screen.findByRole("button", { name: "Set up" }));
    await waitFor(() => expect(document.querySelector("[data-driver-app=codex]")?.textContent).toContain("Uses Cua Driver in Cua Spaces."));
    expect(demo.state.driver.apps.find((a) => a.id === "t3-code")?.on).toBe(false);
    expect(demo.state.driver.lastTest?.passed).toBe(true);
    expect(await screen.findByRole("button", { name: "Repair" })).toBeTruthy();
  });

  it("says a permission is missing, and Allow opens Cua Spaces' guide", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    demo.state.host.permissions = [
      {
        id: "screen-recording",
        label: "Screen Recording",
        instructions: "",
        settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        granted: false,
      },
    ];
    const calls: string[] = [];
    const spy = Object.assign(Object.create(demo) as DataAdapter, {
      call: ((op: string, args: unknown) => {
        calls.push(op);
        return (demo.call as (o: string, a: unknown) => Promise<unknown>).call(demo, op, args);
      }) as DataAdapter["call"],
    });
    mount(spy, () => <DriverPage machine={null} />);
    expect(await screen.findByText("A permission is missing. Allow it below.")).toBeTruthy();
    await waitFor(() => expect(document.querySelector("[data-app-permission=screen]")?.textContent).toContain("Off"));
    // The app the grant goes to, by name, and that the driver needs no second one.
    expect(document.querySelector("[data-app-permission=screen]")?.textContent).toContain("Screen Recording, given to Cua Spaces");
    expect(document.body.textContent).not.toContain("It never asks for its own.");
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(calls).toContain("host.openSettings"));
    // The demo plays the grant in System Settings: the row turns On without another click.
    await waitFor(() => expect(document.querySelector("[data-app-permission=screen]")?.textContent).toContain("On"), { timeout: 5_000 });
  });
});

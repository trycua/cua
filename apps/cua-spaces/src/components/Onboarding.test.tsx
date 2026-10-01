// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { createFakeHostBridge } from "../native/host";
import { Onboarding } from "./Onboarding";

describe("Onboarding", () => {
  it("offers both choices; 'Access other machines' installs nothing", async () => {
    const host = createFakeHostBridge();
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} />);
    expect(screen.getByText("Set up this machine for unattended access")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Access other machines"));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("client", undefined));
    expect(host.calls).toEqual(["onboarding:client"]);
  });

  it("host path joins the relay by default and completes as host", async () => {
    const host = createFakeHostBridge({ platform: "linux" });
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} identity="ada@example.com" />);
    fireEvent.click(screen.getByText("Set up this machine for unattended access"));
    expect(screen.getByText("Joins the Cua relay as ada@example.com. No port forwarding.")).toBeInTheDocument();
    // Direct ip:port is hidden until Advanced is opened.
    expect(screen.queryByText(/Direct connection/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Set up for access" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("host", expect.objectContaining({ configured: true })));
    expect(host.calls).toEqual(["setup:relay:https://relay.cua.ai", "onboarding:host"]);
  });

  it("Advanced exposes direct ip:port and validates it", async () => {
    const host = createFakeHostBridge({ platform: "linux" });
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} />);
    fireEvent.click(screen.getByText("Set up this machine for unattended access"));
    fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
    fireEvent.click(screen.getByLabelText(/Direct connection/));
    const listen = screen.getByDisplayValue("0.0.0.0:3211");
    fireEvent.change(listen, { target: { value: "not an address" } });
    expect(screen.getByRole("button", { name: "Set up for access" })).toBeDisabled();
    fireEvent.change(listen, { target: { value: "192.168.1.20:3211" } });
    fireEvent.click(screen.getByRole("button", { name: "Set up for access" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("host", expect.anything()));
    expect(host.calls[0]).toBe("setup:direct:192.168.1.20:3211");
  });

  it("shows setup errors inline and stays on the form", async () => {
    const host = createFakeHostBridge({ failSetup: "sign in to Cua first" });
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} installerMode="host" />);
    // Installer preselected host mode: straight to the form.
    fireEvent.click(screen.getByRole("button", { name: "Set up for access" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("sign in to Cua first");
    expect(onDone).not.toHaveBeenCalled();
  });

  it("leaves the telemetry notice to Welcome", () => {
    render(<Onboarding host={createFakeHostBridge()} onDone={vi.fn()} />);
    expect(screen.queryByTestId("telemetry-notice")).toBeNull();
  });
});

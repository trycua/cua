// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { createFakeHostBridge } from "../native/host";
import { Onboarding } from "./Onboarding";

describe("Onboarding", () => {
  it("offers both choices; 'open my other computers' installs nothing", async () => {
    const host = createFakeHostBridge();
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} />);
    expect(screen.getByText("Let my other devices reach this Mac")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("client", undefined));
    expect(host.calls).toEqual(["onboarding:client"]);
  });

  it("always shows an enabled Continue with 'open my other computers' selected, and Enter continues", async () => {
    const host = createFakeHostBridge();
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} />);
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
    expect(screen.getByRole("radio", { name: "Just use this Mac to open my other computers" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByTestId("mode-later")).toHaveTextContent("This machine");
    // Choosing a card only selects it.
    fireEvent.click(screen.getByRole("radio", { name: "Let my other devices reach this Mac" }));
    expect(onDone).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("radio", { name: "Just use this Mac to open my other computers" }));
    fireEvent.keyDown(document.body, { key: "Enter" });
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("client", undefined));
  });

  it("host path joins the relay by default and completes as host", async () => {
    const host = createFakeHostBridge({ platform: "linux" });
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} identity="ada@example.com" />);
    fireEvent.click(screen.getByText("Let my other devices reach this Mac"));
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(screen.getByText("You and the people you share with can see and use this computer’s screen from other devices. Cua connects them for you, with no network setup. Signed in as ada@example.com.")).toBeInTheDocument();
    // Direct ip:port is hidden until Advanced is opened.
    expect(screen.queryByText(/Direct connection/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Turn on remote access" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("host", expect.objectContaining({ configured: true })));
    expect(host.calls).toEqual(["setup:relay:https://relay.cua.ai", "onboarding:host"]);
  });

  it("Advanced exposes direct ip:port and validates it", async () => {
    const host = createFakeHostBridge({ platform: "linux" });
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} />);
    fireEvent.click(screen.getByText("Let my other devices reach this Mac"));
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
    fireEvent.click(screen.getByLabelText(/Direct connection/));
    const listen = screen.getByDisplayValue("0.0.0.0:3211");
    fireEvent.change(listen, { target: { value: "not an address" } });
    expect(screen.getByRole("button", { name: "Turn on remote access" })).toBeDisabled();
    fireEvent.change(listen, { target: { value: "192.168.1.20:3211" } });
    fireEvent.click(screen.getByRole("button", { name: "Turn on remote access" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("host", expect.anything()));
    expect(host.calls[0]).toBe("setup:direct:192.168.1.20:3211");
  });

  it("shows setup errors inline and stays on the form", async () => {
    const host = createFakeHostBridge({ failSetup: "sign in to Cua first" });
    const onDone = vi.fn();
    render(<Onboarding host={host} onDone={onDone} installerMode="host" />);
    // Installer preselected host mode: straight to the form.
    fireEvent.click(screen.getByRole("button", { name: "Turn on remote access" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("sign in to Cua first");
    expect(onDone).not.toHaveBeenCalled();
  });

  it("leaves the telemetry notice to Welcome", () => {
    render(<Onboarding host={createFakeHostBridge()} onDone={vi.fn()} />);
    expect(screen.queryByTestId("telemetry-notice")).toBeNull();
  });
});

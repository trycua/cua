// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createFakeHostBridge } from "../../native/host";
import { createFakeInstallerBridge } from "../../native/installer";
import { createAgentsBridgeOver } from "../../native/persistent";
import { fakeFleetBridge } from "../../test/fakeFleet";
import { MainWindow } from "./MainWindow";

/** The shell's events, as the main window listens to them. */
const handlers = new Map<string, () => void>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, handler: () => void) => {
    handlers.set(name, handler);
    return () => handlers.delete(name);
  },
  emit: async () => {},
}));

function setup() {
  const fleet = fakeFleetBridge({
    isNative: true,
    listSpaces: async () => [],
    status: async () => ({ configured: true, authMode: "user", baseUrl: "", tokenUrl: "", identity: "ada@example.com" }),
  });
  const host = createFakeHostBridge({ onboarding: { completed: true, mode: "client" } });
  const agents = createAgentsBridgeOver(async (tool) => {
    if (tool === "volume_requests") return { requests: [] };
    if (tool === "volume_grants") return { grants: [] };
    if (tool === "notifications_list") return { notifications: [] };
    throw new Error(`unknown tool ${tool}`);
  });
  render(
    <MainWindow
      fleet={fleet}
      host={host}
      installer={createFakeInstallerBridge({ plan: { installed: true, upToDate: true, onPath: true } })}
      agents={agents}
      now={() => 1_000}
    />,
  );
}

afterEach(() => window.localStorage.removeItem("cua.settings.experiments"));

describe("MainWindow and the menu bar item", () => {
  it("has no Volume page without the Cua Volume experiment, and main:volume shows the Spaces", async () => {
    setup();
    await waitFor(() => expect(handlers.has("main:volume")).toBe(true));
    const list = within(screen.getByRole("list", { name: "Agents" }));
    expect(list.queryByRole("button", { name: "Volume" })).toBeNull();
    act(() => handlers.get("main:volume")!());
    expect(screen.queryByRole("region", { name: "Grants" })).toBeNull();
  });

  it("opens the Volume page for Cua Volume's conflicts (main:volume)", async () => {
    window.localStorage.setItem("cua.settings.experiments", JSON.stringify({ cuaVolume: true }));
    const fleet = fakeFleetBridge({
      isNative: true,
      listSpaces: async () => [],
      status: async () => ({ configured: true, authMode: "user", baseUrl: "", tokenUrl: "", identity: "ada@example.com" }),
    });
    const host = createFakeHostBridge({ onboarding: { completed: true, mode: "client" } });
    const agents = createAgentsBridgeOver(async (tool) => {
      if (tool === "volume_requests") return { requests: [] };
      if (tool === "volume_grants") return { grants: [] };
      if (tool === "notifications_list") return { notifications: [] };
      throw new Error(`unknown tool ${tool}`);
    });
    render(
      <MainWindow
        fleet={fleet}
        host={host}
        installer={createFakeInstallerBridge({ plan: { installed: true, upToDate: true, onPath: true } })}
        agents={agents}
        now={() => 1_000}
      />,
    );
    await waitFor(() => expect(handlers.has("main:volume")).toBe(true));
    const nav = () => within(screen.getByRole("list", { name: "Agents" })).getByRole("button", { name: "Volume" });
    expect(nav()).toHaveAttribute("aria-selected", "false");
    act(() => handlers.get("main:volume")!());
    await waitFor(() => expect(nav()).toHaveAttribute("aria-selected", "true"));
    expect(await screen.findByRole("region", { name: "Grants" })).toBeInTheDocument();
  });
});

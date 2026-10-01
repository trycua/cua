// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { FleetBridge, ViewerConfig } from "../native/fleet";
import type { TransferBridge } from "../native/transfer";
import { fakeFleetBridge } from "../test/fakeFleet";
import { SpaceViewer } from "./SpaceViewer";

const SPACE = {
  id: "cloud:aurora-1",
  name: "Aurora",
};

function makeFleet(overrides: Partial<FleetBridge> = {}): FleetBridge {
  const config: ViewerConfig = { view: "space", space: SPACE };
  return fakeFleetBridge({
    // A Space without desktop_stream shows the "update the image" message
    // instead of streaming, so the bar renders without a WebSocket.
    listSpaces: async () => [
      {
        id: SPACE.id,
        name: "aurora-1",
        provider: "cloud",
        spacesdVersion: "0",
        features: [],
        reachable: true,
      },
    ],
    viewerConfig: async () => config,
    openTeleportPicker: vi.fn(async () => {}),
    launchAgent: vi.fn(async () => {}),
    launchAgentTerminal: vi.fn(async () => {}),
    ...overrides,
  });
}

const TRANSFER: TransferBridge = {
  isNative: false,
  begin: async () => {},
  update: async () => {},
  retry: async () => {},
  cancel: async () => {},
  onTransfer: async () => () => {},
};

async function openMenu(fleet: FleetBridge) {
  render(<SpaceViewer fleet={fleet} transfer={TRANSFER} />);
  const trigger = await screen.findByRole("button", { name: /launch agent/i });
  fireEvent.click(trigger);
  return trigger;
}

describe("SpaceViewer Launch Agent dropdown", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("opens the menu and lists every launchable coding agent in order", async () => {
    await openMenu(makeFleet());
    const items = screen.getAllByRole("menuitem").map((el) => el.textContent);
    expect(items).toEqual([
      "Claude Code",
      "OpenAI Codex",
      "Gemini CLI",
      "OpenCode",
      "Goose",
      "Pi",
      "Hermes",
      "OpenClaw",
    ]);
  });

  it("does not offer Google Antigravity, which has no interactive terminal", async () => {
    await openMenu(makeFleet());
    expect(screen.queryByRole("menuitem", { name: /antigravity/i })).toBeNull();
  });

  it("launches Claude Code via its teleport provider", async () => {
    const fleet = makeFleet();
    await openMenu(fleet);
    fireEvent.click(screen.getByRole("menuitem", { name: "Claude Code" }));

    expect(fleet.launchAgent).toHaveBeenCalledTimes(1);
    expect(fleet.launchAgent).toHaveBeenCalledWith(
      expect.objectContaining({ id: SPACE.id }),
      "claude-code",
      "Claude Code",
    );
    expect(fleet.launchAgentTerminal).not.toHaveBeenCalled();
    // Selecting closes the menu.
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it.each([
    ["OpenAI Codex", "openai-codex"],
    ["Gemini CLI", "gemini-cli"],
    ["OpenCode", "opencode"],
    ["Goose", "goose"],
    ["Pi", "pi"],
    ["Hermes", "hermes"],
    ["OpenClaw", "openclaw"],
  ])("opens %s in a terminal in the Space", async (name, harness) => {
    const fleet = makeFleet();
    await openMenu(fleet);
    fireEvent.click(screen.getByRole("menuitem", { name }));

    expect(fleet.launchAgentTerminal).toHaveBeenCalledTimes(1);
    expect(fleet.launchAgentTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ id: SPACE.id }),
      harness,
    );
    expect(fleet.launchAgent).not.toHaveBeenCalled();
    expect(
      await screen.findByText(`${name} is open in a terminal in this Space; sign in there`),
    ).toBeInTheDocument();
  });

  it("keeps an installing notice up until the terminal opens", async () => {
    let finish: () => void = () => {};
    const fleet = makeFleet({
      launchAgentTerminal: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            finish = resolve;
          }),
      ),
    });
    await openMenu(fleet);
    fireEvent.click(screen.getByRole("menuitem", { name: "Goose" }));
    expect(await screen.findByText("Installing Goose in this Space…")).toBeInTheDocument();

    await act(async () => finish());
    expect(
      await screen.findByText("Goose is open in a terminal in this Space; sign in there"),
    ).toBeInTheDocument();
  });

  it("says why when a terminal launch fails", async () => {
    const fleet = makeFleet({
      launchAgentTerminal: vi.fn(async () => {
        throw new Error("this Space has no xterm to open it in");
      }),
    });
    await openMenu(fleet);
    fireEvent.click(screen.getByRole("menuitem", { name: "Hermes" }));
    expect(
      await screen.findByText("Couldn't open Hermes: this Space has no xterm to open it in"),
    ).toBeInTheDocument();
  });
});

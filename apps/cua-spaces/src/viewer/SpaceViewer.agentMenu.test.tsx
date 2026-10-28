// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SpacesBridge, ViewerConfig } from "../native/spacesBridge";
import type { TransferBridge } from "../native/transfer";
import { fakeSpacesBridge } from "../test/fakeSpacesBridge";
import { SpaceViewer } from "./SpaceViewer";

const SPACE = {
  id: "direct:aurora-1",
  name: "Aurora",
};

function makeBridge(overrides: Partial<SpacesBridge> = {}): SpacesBridge {
  const config: ViewerConfig = { view: "space", space: SPACE };
  return fakeSpacesBridge({
    // A Space without desktop_stream shows the "update the image" message
    // instead of streaming, so the bar renders without a WebSocket.
    listSpaces: async () => [
      {
        id: SPACE.id,
        name: "aurora-1",
        provider: "direct",
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

async function openMenu(bridge: SpacesBridge) {
  render(<SpaceViewer bridge={bridge} transfer={TRANSFER} />);
  const trigger = await screen.findByRole("button", { name: /launch agent/i });
  fireEvent.click(trigger);
  return trigger;
}

describe("SpaceViewer Launch Agent dropdown", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("opens the menu and lists every launchable coding agent in order", async () => {
    await openMenu(makeBridge());
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
    await openMenu(makeBridge());
    expect(screen.queryByRole("menuitem", { name: /antigravity/i })).toBeNull();
  });

  it("launches Claude Code via its teleport provider", async () => {
    const bridge = makeBridge();
    await openMenu(bridge);
    fireEvent.click(screen.getByRole("menuitem", { name: "Claude Code" }));

    expect(bridge.launchAgent).toHaveBeenCalledTimes(1);
    expect(bridge.launchAgent).toHaveBeenCalledWith(
      expect.objectContaining({ id: SPACE.id }),
      "claude-code",
      "Claude Code",
    );
    expect(bridge.launchAgentTerminal).not.toHaveBeenCalled();
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
    const bridge = makeBridge();
    await openMenu(bridge);
    fireEvent.click(screen.getByRole("menuitem", { name }));

    expect(bridge.launchAgentTerminal).toHaveBeenCalledTimes(1);
    expect(bridge.launchAgentTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ id: SPACE.id }),
      harness,
    );
    expect(bridge.launchAgent).not.toHaveBeenCalled();
    expect(
      await screen.findByText(`${name} is open in a terminal in this Space; sign in there`),
    ).toBeInTheDocument();
  });

  it("keeps an installing notice up until the terminal opens", async () => {
    let finish: () => void = () => {};
    const bridge = makeBridge({
      launchAgentTerminal: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            finish = resolve;
          }),
      ),
    });
    await openMenu(bridge);
    fireEvent.click(screen.getByRole("menuitem", { name: "Goose" }));
    expect(await screen.findByText("Installing Goose in this Space…")).toBeInTheDocument();

    await act(async () => finish());
    expect(
      await screen.findByText("Goose is open in a terminal in this Space; sign in there"),
    ).toBeInTheDocument();
  });

  it("says why when a terminal launch fails", async () => {
    const bridge = makeBridge({
      launchAgentTerminal: vi.fn(async () => {
        throw new Error("this Space has no xterm to open it in");
      }),
    });
    await openMenu(bridge);
    fireEvent.click(screen.getByRole("menuitem", { name: "Hermes" }));
    expect(
      await screen.findByText("Couldn't open Hermes: this Space has no xterm to open it in"),
    ).toBeInTheDocument();
  });
});

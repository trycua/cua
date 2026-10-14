// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { SpaceRow } from "../model/spaces";
import type { FleetBridge, ViewerConfig } from "../native/fleet";
import type { TransferBridge } from "../native/transfer";
import { fakeFleetBridge } from "../test/fakeFleet";
import { hasDesktopStream, NO_DESKTOP_STREAM_MESSAGE, phaseForStatus, SpaceViewer } from "./SpaceViewer";

/** Records every WebSocket the viewer opens (never connects anywhere). */
class RecordingSocket {
  static OPEN = 1;
  static urls: string[] = [];
  readyState = 0;
  binaryType = "";
  onmessage: unknown = null;
  onclose: unknown = null;
  onerror: unknown = null;
  constructor(url: string, readonly protocols?: string[]) {
    RecordingSocket.urls.push(url);
  }
  send() {}
  close() {}
}

function row(id: string, features: string[]): SpaceRow {
  return { id, name: "x", provider: "direct", spacesdVersion: "0", features, reachable: true };
}

function fleetWith(config: ViewerConfig, rows: SpaceRow[], extra: Partial<FleetBridge> = {}) {
  const openStream = vi.fn(async () => ({
    spaceId: config.space.id,
    mediaSessionId: "m-1",
    wsUrl: "ws://10.0.0.5:3211/media?ticket=abc",
    ticket: "abc",
    codec: "h264" as const,
    wireVersion: 2,
    frameSize: [1280, 800] as [number, number],
    via: "direct" as const,
    audio: true,
  }));
  const fleet = fakeFleetBridge({
    isNative: true,
    viewerConfig: async () => config,
    listSpaces: async () => rows,
    openStream,
    ...extra,
  });
  return { fleet, openStream };
}

const TRANSFER: TransferBridge = {
  isNative: false,
  begin: async () => {},
  update: async () => {},
  retry: async () => {},
  cancel: async () => {},
  onTransfer: async () => () => {},
};

let originalWebSocket: unknown;
beforeEach(() => {
  RecordingSocket.urls = [];
  originalWebSocket = (globalThis as { WebSocket?: unknown }).WebSocket;
  (globalThis as { WebSocket: unknown }).WebSocket = RecordingSocket;
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
    null as unknown as CanvasRenderingContext2D,
  );
});
afterEach(() => {
  (globalThis as { WebSocket?: unknown }).WebSocket = originalWebSocket;
  vi.restoreAllMocks();
});

describe("SpaceViewer transport", () => {
  it("streams the spacesd DESKTOP when the Space has desktop_stream", async () => {
    const config: ViewerConfig = { view: "space", space: { id: "space://direct/10.0.0.5:3211", name: "Box" } };
    const { fleet, openStream } = fleetWith(config, [row(config.space.id, ["desktop_stream"])]);
    render(<SpaceViewer fleet={fleet} transfer={TRANSFER} />);
    await waitFor(() => expect(RecordingSocket.urls).toContain("ws://10.0.0.5:3211/media?ticket=abc"));
    expect(openStream).toHaveBeenCalledWith(config.space.id, { kind: "display" }, { audio: true });
  });

  it("opens the PiP mirror view-only with no audio", async () => {
    const config: ViewerConfig = { view: "pip", space: { id: "space://direct/h:1", name: "Box" } };
    const { fleet, openStream } = fleetWith(config, [row(config.space.id, ["desktop_stream"])]);
    render(<SpaceViewer fleet={fleet} transfer={TRANSFER} />);
    await waitFor(() => expect(openStream).toHaveBeenCalled());
    expect(openStream).toHaveBeenCalledWith(
      config.space.id,
      { kind: "display" },
      expect.objectContaining({ policy: "view_only", audio: false }),
    );
  });

  it("shows an update-the-image message when the Space lacks desktop_stream", async () => {
    const config: ViewerConfig = { view: "space", space: { id: "space://direct/10.0.0.5:3211", name: "Old" } };
    const { fleet, openStream } = fleetWith(config, [row(config.space.id, ["window_stream"])]);
    render(<SpaceViewer fleet={fleet} transfer={TRANSFER} />);
    expect(await screen.findByRole("alert")).toHaveTextContent(NO_DESKTOP_STREAM_MESSAGE);
    expect(openStream).not.toHaveBeenCalled();
    expect(RecordingSocket.urls).toEqual([]);
  });
});

describe("hasDesktopStream / phaseForStatus", () => {
  it("prefers the stream when the registry is unavailable or the Space is unknown", async () => {
    const failing = fakeFleetBridge({ listSpaces: async () => Promise.reject(new Error("down")) });
    await expect(hasDesktopStream(failing, "space://direct/a:1")).resolves.toBe(true);
    const empty = fakeFleetBridge({ listSpaces: async () => [] });
    await expect(hasDesktopStream(empty, "space://direct/a:1")).resolves.toBe(true);
    const old = fakeFleetBridge({ listSpaces: async () => [row("space://direct/a:1", ["window_stream"])] });
    await expect(hasDesktopStream(old, "space://direct/a:1")).resolves.toBe(false);
  });

  it("maps media statuses onto viewer phases", () => {
    expect(phaseForStatus("streaming")).toBe("connected");
    expect(phaseForStatus("reconnecting")).toBe("connecting");
    expect(phaseForStatus("ended")).toBe("disconnected");
    expect(phaseForStatus("failed")).toBe("failed");
  });
});

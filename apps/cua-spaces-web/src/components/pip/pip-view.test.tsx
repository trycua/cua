// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CuaDesktopBridge } from "@/bridge/detect";
import { resetVideoSlotLoader } from "@/components/video/use-video-slot";
import type { SessionFactory } from "@/components/video/webcodecs-slots";
import { resetWebCodecsSlots } from "@/components/video/webcodecs-slots";

import { pipBadge, pipParams, PipView } from "./pip-view";

// The production MediaSession, replaced: jsdom has no WebSocket media or VideoDecoder.
const sessions: { options: Parameters<SessionFactory>[0] }[] = [];
vi.mock("../../../../../libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession", () => ({
  MediaSession: class {
    constructor(options: Parameters<SessionFactory>[0]) {
      sessions.push({ options });
    }
    start() {}
    stop() {}
  },
}));

type Call = { method: string; args: Record<string, unknown> };

function electron(platform = "darwin") {
  const calls: Call[] = [];
  const desktop: CuaDesktopBridge = {
    invoke: async (_channel, request) => {
      const r = request as { id: string } & Call;
      calls.push({ method: r.method, args: r.args });
      return { id: r.id, ok: true, result: r.method === "spaces.openStream" ? { wsUrl: "ws://127.0.0.1:1/m", expiresAt: null } : null };
    },
    platform,
  };
  (window as { cuaDesktop?: unknown }).cuaDesktop = desktop;
  vi.stubGlobal("VideoDecoder", class {});
  return calls;
}

beforeEach(() => {
  sessions.length = 0;
  resetVideoSlotLoader();
  resetWebCodecsSlots();
});
afterEach(() => {
  cleanup();
  delete (window as { cuaDesktop?: unknown }).cuaDesktop;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("the picture-in-picture view's address", () => {
  it("reads the Space, its window and epoch", () => {
    expect(pipParams("?space=local%3Aa&name=Aurora&title=Terminal&key=window%3Aw-1&os=linux&window=w-1&epoch=3")).toEqual({
      spaceId: "local:a",
      spaceName: "Aurora",
      title: "Terminal",
      os: "linux",
      windowId: "w-1",
      epoch: 3,
    });
    expect(pipParams("?space=local%3Aa&title=Desktop")).toEqual({ spaceId: "local:a", spaceName: "", title: "Desktop", os: undefined, windowId: undefined, epoch: undefined });
  });

  it("shows the Swift panel's status chip until the stream is live", () => {
    expect(pipBadge("connecting")).toEqual({ text: "Connecting…", tone: "yellow" });
    expect(pipBadge("live")).toBeNull();
    expect(pipBadge("failed", "the window is gone")).toEqual({ text: "Failed: the window is gone", tone: "red" });
    expect(pipBadge(null)?.tone).toBe("red");
  });
});

describe("<PipView>", () => {
  const params = { spaceId: "local:a", spaceName: "Aurora", title: "Terminal", os: "linux", windowId: "w-1", epoch: 3 };

  it("streams the window with input, takes the stream's shape once live, and says Connecting… until then", async () => {
    const calls = electron();
    const resize = vi.spyOn(window, "resizeTo").mockImplementation(() => {});
    const { container, getByText, queryByText } = render(<PipView params={params} />);
    expect(getByText("Connecting…")).toBeTruthy();
    await waitFor(() => expect(sessions).toHaveLength(1));
    expect(calls[0]).toEqual({ method: "spaces.openStream", args: { spaceId: "local:a", tier: "full", windowId: "w-1", epoch: 3 } });
    expect(sessions[0]!.options).toMatchObject({ interactive: true, metaAsControl: true, hardwareAcceleration: "prefer-hardware" });
    const canvas = container.querySelector<HTMLCanvasElement>("[data-pip-stream] canvas")!;
    canvas.width = 800;
    canvas.height = 1200;
    act(() => sessions[0]!.options.onFrame?.(800, 1200));
    await waitFor(() => expect(queryByText("Connecting…")).toBeNull());
    expect(resize).toHaveBeenCalledWith(800, 1200);
    expect(document.title).toBe("Terminal");
  });

  it("opens the Space and closes the panel from its bar", async () => {
    const calls = electron();
    const close = vi.spyOn(window, "close").mockImplementation(() => {});
    const { getByLabelText } = render(<PipView params={params} />);
    fireEvent.click(getByLabelText("Open Space"));
    await waitFor(() => expect(calls.some((c) => c.method === "spaces.open")).toBe(true));
    expect(calls.find((c) => c.method === "spaces.open")?.args).toEqual({ id: "local:a" });
    fireEvent.click(getByLabelText("Close"));
    expect(close).toHaveBeenCalled();
  });

  it("offers Try again when the stream fails", async () => {
    const calls = electron();
    const { findByText } = render(<PipView params={params} />);
    await waitFor(() => expect(sessions).toHaveLength(1));
    act(() => sessions[0]!.options.onGone?.("the window is gone"));
    fireEvent.click(await findByText("Try again"));
    await waitFor(() => expect(sessions).toHaveLength(2));
    expect(calls.filter((c) => c.method === "spaces.openStream")).toHaveLength(2);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { RemoteWindow } from "../model/teleport";
import type { ViewerConfig } from "../native/fleet";
import { fakeFleetBridge } from "../test/fakeFleet";
import { avcCodecFromAnnexB } from "@cua/spacesd-html5/core/h264";
import {
  applyCursorShape,
  cursorShapeToCss,
  MAX_WINDOW_SESSIONS,
  PRIMARY_WINDOW_OPTS,
  REPLICA_OPTS,
  selectWindows,
  WINDOW_POLL_MS,
  WindowStream,
} from "./WindowStream";

describe("avcCodecFromAnnexB", () => {
  it("reads profile/constraints/level from the SPS of an Annex-B unit", () => {
    // 4-byte start code, NAL type 7 (SPS), then profile=0x64 constraints=0x00 level=0x28.
    const data = new Uint8Array([0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xff]);
    expect(avcCodecFromAnnexB(data)).toBe("avc1.640028");
  });

  it("finds the SPS after a leading 3-byte-start NAL", () => {
    const data = new Uint8Array([
      0, 0, 1, 0x09, 0x10, // access unit delimiter (type 9)
      0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, // SPS (type 7)
    ]);
    expect(avcCodecFromAnnexB(data)).toBe("avc1.42E01F");
  });

  it("returns null when there is no SPS", () => {
    const data = new Uint8Array([0, 0, 0, 1, 0x41, 0x9a, 0x00]); // type 1, non-IDR slice
    expect(avcCodecFromAnnexB(data)).toBeNull();
  });
});

describe("cursorShapeToCss", () => {
  it("maps the semantic shapes onto native CSS cursors", () => {
    expect(cursorShapeToCss({ kind: "text" })).toBe("text");
    expect(cursorShapeToCss({ kind: "pointer" })).toBe("pointer");
    expect(cursorShapeToCss({ kind: "grabbing" })).toBe("grabbing");
    expect(cursorShapeToCss({ kind: "not_allowed" })).toBe("not-allowed");
    expect(cursorShapeToCss({ kind: "resize", axis: "north_south" })).toBe("ns-resize");
    expect(cursorShapeToCss({ kind: "resize", axis: "north_west_south_east" })).toBe(
      "nwse-resize",
    );
    expect(cursorShapeToCss({ kind: "resize", axis: "column" })).toBe("col-resize");
  });

  // The distinction the whole feature rests on. `unknown` means the host cannot
  // report a shape; rendering it as an arrow would be a confident lie, and
  // would be indistinguishable from a genuine `default`.
  it("treats unknown as 'hold the previous shape', NOT as an arrow", () => {
    expect(cursorShapeToCss({ kind: "unknown" })).toBeNull();
    expect(cursorShapeToCss({ kind: "default" })).toBe("default");
    expect(cursorShapeToCss({ kind: "unknown" })).not.toBe(
      cursorShapeToCss({ kind: "default" }),
    );
  });

  it("holds the previous shape for a shape introduced after this build", () => {
    expect(cursorShapeToCss({ kind: "holographic_pointer" })).toBeNull();
  });

  it("renders a custom guest cursor from its own pixels and hot spot", () => {
    const css = cursorShapeToCss({
      kind: "custom",
      png: [137, 80, 78, 71],
      hotspot_x: 4.4,
      hotspot_y: 7.6,
      scale: 2,
    });
    expect(css).toContain("url(data:image/png;base64,");
    // Hot spot is rounded to whole CSS pixels, and a fallback keyword is kept
    // so a rejected data URL still yields a usable cursor.
    expect(css).toContain(") 4 8, default");
  });

  it("ignores a malformed shape rather than throwing", () => {
    expect(cursorShapeToCss(null)).toBeNull();
    expect(cursorShapeToCss("text")).toBeNull();
    expect(cursorShapeToCss({ kind: "custom", png: [] })).toBeNull();
  });
});

describe("applyCursorShape", () => {
  it("applies a shape and leaves the previous one alone when the host says unknown", () => {
    const el = document.createElement("div");
    applyCursorShape(el, { kind: "text" });
    expect(el.style.cursor).toBe("text");
    expect(el.dataset.cursorShape).toBe("text");

    // An `unknown` update must NOT reset the element to an arrow.
    applyCursorShape(el, { kind: "unknown" });
    expect(el.style.cursor).toBe("text");
    expect(el.dataset.cursorShape).toBe("text");

    applyCursorShape(el, { kind: "pointer" });
    expect(el.style.cursor).toBe("pointer");
  });
});

// --- WindowStream windows (single + multi) -----------------------------------

class RecordingSocket {
  static OPEN = 1;
  static urls: string[] = [];
  readyState = 0;
  binaryType = "";
  onmessage: unknown = null;
  onclose: unknown = null;
  onerror: unknown = null;
  constructor(url: string) {
    RecordingSocket.urls.push(url);
  }
  send() {}
  close() {}
}

function win(id: string, visible = true): RemoteWindow {
  return { id, appName: "Firefox", title: `Tab ${id}`, visible, appId: "firefox", targetEpoch: 1 };
}

describe("WindowStream", () => {
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
    vi.useRealTimers();
  });

  it("streams the SDK's windows in order, capped", () => {
    const many = Array.from({ length: 40 }, (_, i) => win(String(i)));
    const picked = selectWindows(many);
    expect(picked).toHaveLength(MAX_WINDOW_SESSIONS);
    expect(picked.map((w) => w.id)).toEqual(many.slice(0, MAX_WINDOW_SESSIONS).map((w) => w.id));
  });

  it("single mode attaches an MCP-provided ticket as-is (no open_space_stream)", async () => {
    const openStream = vi.fn();
    const config: ViewerConfig = {
      view: "windows",
      space: { id: "space://direct/h:1", name: "Box" },
      targetWindow: {
        id: "w-7",
        appName: "Firefox",
        title: "Docs",
        mediaUrl: "ws://h:1/media?ticket=from-mcp",
        mediaSessionId: "m-9",
      },
    };
    render(<WindowStream fleet={fakeFleetBridge({ viewerConfig: async () => config, openStream })} />);
    await screen.findByText(/Connecting to Firefox/);
    // The canvas opens its socket after the status text renders.
    await vi.waitFor(() => expect(RecordingSocket.urls).toEqual(["ws://h:1/media?ticket=from-mcp"]));
    expect(openStream).not.toHaveBeenCalled();
  });

  it("single mode opens a window session with geometry control; a replica never asks for h264", async () => {
    const ticket = {
      spaceId: "s",
      mediaSessionId: "m",
      wsUrl: "ws://h:1/media?ticket=t",
      ticket: "t",
      codec: "h264" as const,
      wireVersion: 2,
      frameSize: [1, 1] as [number, number],
      via: "direct" as const,
      audio: false,
    };
    const openStream = vi.fn(async () => ticket);
    const primary: ViewerConfig = {
      view: "windows",
      space: { id: "space://direct/h:1", name: "Box" },
      targetWindow: { id: "w-7", appName: "Firefox", title: "Docs" },
    };
    const { unmount } = render(
      <WindowStream fleet={fakeFleetBridge({ viewerConfig: async () => primary, openStream })} />,
    );
    await vi.waitFor(() => expect(openStream).toHaveBeenCalled());
    expect(openStream).toHaveBeenLastCalledWith(
      "space://direct/h:1",
      { kind: "window", windowId: "w-7" },
      { ...PRIMARY_WINDOW_OPTS, geometryControl: true },
    );
    expect(PRIMARY_WINDOW_OPTS.codecs![0]).toBe("h264");
    unmount();

    const replica: ViewerConfig = { ...primary, targetWindow: { ...primary.targetWindow!, replica: true } };
    render(<WindowStream fleet={fakeFleetBridge({ viewerConfig: async () => replica, openStream })} />);
    await vi.waitFor(() => expect(openStream).toHaveBeenCalledTimes(2));
    expect(openStream).toHaveBeenLastCalledWith(
      "space://direct/h:1",
      { kind: "window", windowId: "w-7" },
      { ...REPLICA_OPTS, geometryControl: false },
    );
    expect(REPLICA_OPTS.codecs).not.toContain("h264");
  });

  it("multi mode polls the window list and opens one session per window", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let windows = [win("a"), win("b")];
    const listWindows = vi.fn(async () => windows);
    const openStream = vi.fn(async (_space: string, target: { kind: string; windowId?: string }) => ({
      spaceId: "s",
      mediaSessionId: `m-${target.windowId}`,
      wsUrl: `ws://h:1/media?ticket=${target.windowId}`,
      ticket: String(target.windowId),
      codec: "h264" as const,
      wireVersion: 2,
      frameSize: [1, 1] as [number, number],
      via: "direct" as const,
      audio: false,
    }));
    const config: ViewerConfig = { view: "windows", space: { id: "space://direct/h:1", name: "Box" } };
    render(
      <WindowStream
        fleet={fakeFleetBridge({ viewerConfig: async () => config, openStream: openStream as never })}
        listWindows={listWindows}
      />,
    );
    await vi.waitFor(() => expect(RecordingSocket.urls).toHaveLength(2));
    expect(screen.getByText("Firefox — Tab a")).toBeInTheDocument();
    windows = [win("a"), win("b"), win("c")];
    await act(async () => {
      await vi.advanceTimersByTimeAsync(WINDOW_POLL_MS + 10);
    });
    await vi.waitFor(() => expect(RecordingSocket.urls).toHaveLength(3));
    expect(RecordingSocket.urls).toContain("ws://h:1/media?ticket=c");
  });
});

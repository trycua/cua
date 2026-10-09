// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CuaDesktopBridge } from "@/bridge/detect";
import { resetVideoSlotLoader } from "@/components/video/use-video-slot";
import type { SessionFactory } from "@/components/video/webcodecs-slots";
import { resetWebCodecsSlots } from "@/components/video/webcodecs-slots";

import { dimensionsLabel, sourceLabel, sourceRow, viewerParams, ViewerView } from "./viewer-view";

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

const TERMINAL = { id: "w-1", appName: "Terminal", title: "cua@space: ~", visible: true, appId: "org.gnome.Terminal", targetEpoch: 3, widthPx: 800, heightPx: 600, pid: 9 };

/** The Electron preload, answering as the shell's bridge does; `open` is the host's panels. */
function electron() {
  const calls: Call[] = [];
  const host = { open: [] as string[] };
  const desktop: CuaDesktopBridge = {
    invoke: async (_channel, request) => {
      const r = request as { id: string } & Call;
      calls.push({ method: r.method, args: r.args });
      let result: unknown = null;
      if (r.method === "spaces.openStream") result = { wsUrl: "ws://127.0.0.1:1/m", expiresAt: null };
      if (r.method === "spaces.windows") result = { windows: [TERMINAL], display: { widthPx: 1280, heightPx: 800 }, open: [...host.open] };
      if (r.method === "stream.pip") {
        const { type, row } = r.args.command as { type: string; row: string };
        host.open = type === "open" ? [...host.open, row] : host.open.filter((x) => x !== row);
        result = [...host.open];
      }
      return { id: r.id, ok: true, result };
    },
    platform: "darwin",
  };
  (window as { cuaDesktop?: unknown }).cuaDesktop = desktop;
  vi.stubGlobal("VideoDecoder", class {});
  vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener() {}, removeEventListener() {} }));
  return { calls, host };
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

describe("the viewer's address and words", () => {
  it("reads the Space, its name and OS", () => {
    expect(viewerParams("?space=local%3Aa&name=Aurora&os=linux")).toEqual({ spaceId: "local:a", spaceName: "Aurora", os: "linux" });
    expect(viewerParams("?space=local%3Aa&name=")).toEqual({ spaceId: "local:a", spaceName: "", os: undefined });
  });

  it("labels the source as the Swift picker does, and the size without the frame count", () => {
    expect(sourceLabel({ kind: "desktop" })).toBe("Full desktop");
    expect(sourceLabel({ kind: "window", window: TERMINAL })).toBe("cua@space: ~");
    expect(sourceLabel({ kind: "window", window: { ...TERMINAL, title: " " } })).toBe("Terminal");
    expect(sourceRow({ kind: "desktop" })).toBe("desktop");
    expect(sourceRow({ kind: "window", window: TERMINAL })).toBe("w-1");
    expect(dimensionsLabel(1280, 800)).toBe("1280×800");
    expect(dimensionsLabel(0, 0)).toBe("—");
  });
});

describe("<ViewerView>", () => {
  const params = { spaceId: "local:a", spaceName: "Aurora", os: "linux" };

  it("streams the desktop with input under its toolbar, titled with the Space, and takes the stream's shape once", async () => {
    const { calls } = electron();
    const resize = vi.spyOn(window, "resizeTo").mockImplementation(() => {});
    const { container, getByText, queryByText } = render(<ViewerView params={params} />);
    expect(getByText("Full desktop")).toBeTruthy();
    expect(getByText("Pop out")).toBeTruthy();
    expect(getByText("—")).toBeTruthy();
    expect(getByText("Connecting…")).toBeTruthy();
    await waitFor(() => expect(sessions).toHaveLength(1));
    expect(calls.find((c) => c.method === "spaces.openStream")?.args).toEqual({ spaceId: "local:a", tier: "full" });
    expect(sessions[0]!.options).toMatchObject({ interactive: true, metaAsControl: true });
    const canvas = container.querySelector<HTMLCanvasElement>("[data-viewer-stream] canvas")!;
    canvas.width = 1280;
    canvas.height = 800;
    act(() => sessions[0]!.options.onFrame?.(1280, 800));
    await waitFor(() => expect(queryByText("Connecting…")).toBeNull());
    expect(getByText("1280×800")).toBeTruthy();
    expect(resize).toHaveBeenCalledWith(1280, 800);
    canvas.width = 1920;
    await waitFor(() => expect(getByText("1920×800")).toBeTruthy());
    expect(resize).toHaveBeenCalledTimes(1);
    expect(document.title).toBe("Aurora");
  });

  it("pops the stream out to a floating window, says so in place, and brings it back", async () => {
    const { calls, host } = electron();
    const { getByText, findByText, container } = render(<ViewerView params={params} />);
    await waitFor(() => expect(sessions).toHaveLength(1));
    fireEvent.click(getByText("Pop out"));
    expect(await findByText("Playing in a floating window")).toBeTruthy();
    expect(calls.find((c) => c.method === "stream.pip")?.args).toEqual({ spaceId: "local:a", command: { type: "open", row: "desktop" } });
    // No second stream here while the panel plays it.
    expect(container.querySelector("[data-viewer-stream]")).toBeNull();
    expect(getByText("Pop in")).toBeTruthy();
    fireEvent.click(getByText("Bring it back"));
    await waitFor(() => expect(host.open).toEqual([]));
    await waitFor(() => expect(container.querySelector("[data-viewer-stream]")).not.toBeNull());
    await waitFor(() => expect(sessions).toHaveLength(2));
  });

  it("follows the host's panels: one opened elsewhere shows here on the next read", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { host } = electron();
      const { findByText } = render(<ViewerView params={params} />);
      host.open = ["desktop"];
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });
      expect(await findByText("Playing in a floating window")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("offers Try again when the stream fails", async () => {
    electron();
    const { findByText } = render(<ViewerView params={params} />);
    await waitFor(() => expect(sessions).toHaveLength(1));
    act(() => sessions[0]!.options.onGone?.("the Space went away"));
    expect(await findByText("Failed: the Space went away")).toBeTruthy();
    fireEvent.click(await findByText("Try again"));
    await waitFor(() => expect(sessions).toHaveLength(2));
  });
});

describe("the source picker", () => {
  it("lists the Space's windows and streams the one picked", async () => {
    const { calls } = electron();
    const { getByText, findByText } = render(<ViewerView params={{ spaceId: "local:a", spaceName: "Aurora", os: "linux" }} />);
    await waitFor(() => expect(calls.some((c) => c.method === "spaces.windows")).toBe(true));
    fireEvent.click(getByText("Full desktop"));
    expect(await findByText("Refresh windows")).toBeTruthy();
    fireEvent.click(await findByText("cua@space: ~"));
    await waitFor(() => expect(calls.filter((c) => c.method === "spaces.openStream").at(-1)?.args).toEqual({ spaceId: "local:a", tier: "full", windowId: "w-1", epoch: 3 }));
  });
});

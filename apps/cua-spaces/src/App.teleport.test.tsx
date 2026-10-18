// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { FIXTURE_NOW } from "./model/fixtures";
import type { NativeBridge } from "./native/bridge";
import type { PortalEnvironment, PortalGeometry, WindowMode } from "./native/types";
import type { WindowDragBridge, WindowDragEvent } from "./native/windowDrag";

// The drag effect only runs for a *native* bridge, which reaches for a couple of
// Tauri window APIs. Stub them so they resolve harmlessly under jsdom.
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    label: "portal",
    innerPosition: async () => ({ x: 0, y: 0 }),
    scaleFactor: async () => 1,
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));

const MONITOR = { x: 0, y: 0, width: 1512, height: 982 };
const SIZES: Record<WindowMode, { w: number; h: number }> = {
  ambient: { w: 420, h: 38 },
  "ambient-teleport": { w: 420, h: 96 },
  switcher: { w: 760, h: 320 },
  "create-fleet": { w: 600, h: 700 },
};

function geom(mode: WindowMode): PortalGeometry {
  const s = SIZES[mode];
  return {
    mode,
    displayStyle: "notched",
    frame: { x: Math.round((MONITOR.width - s.w) / 2), y: 0, width: s.w, height: s.h },
    monitor: MONITOR,
    scaleFactor: 2,
  };
}

function nativeNotchedBridge(): NativeBridge & { modes: WindowMode[] } {
  const modes: WindowMode[] = [];
  const env = (): PortalEnvironment => ({
    platform: "macos",
    displayStyle: "notched",
    displayStyleSource: "override",
    accessoryActivation: true,
    native: true,
    geometry: geom("ambient"),
  });
  return {
    isNative: true,
    modes,
    getEnvironment: async () => env(),
    setWindowMode: async (mode) => {
      modes.push(mode);
      return geom(mode);
    },
    setDisplayStyle: async () => env(),
  };
}

function fakeWindowDrag() {
  let handler: ((event: WindowDragEvent) => void) | null = null;
  const bridge: WindowDragBridge = {
    isNative: true,
    axTrusted: async () => true,
    requestAxTrust: async () => true,
    startWindowDrag: async () => true,
    setForeignWindowHidden: async () => false,
    dragDisplays: async () => [],
    onWindowDrag: async (h) => {
      handler = h;
      return () => {
        handler = null;
      };
    },
    onPermission: async () => () => {},
  };
  return {
    bridge,
    ready: () => handler !== null,
    emit: (event: WindowDragEvent) =>
      act(() => {
        handler?.(event);
      }),
  };
}

describe("teleport morph: tab → notch box → selector", () => {
  it("collapses the tab into the notch box, then expands the Space selector", async () => {
    const bridge = nativeNotchedBridge();
    const drag = fakeWindowDrag();
    let clock = FIXTURE_NOW;
    render(<App bridge={bridge} windowDrag={drag.bridge} now={() => (clock += 1000)} />);

    // Idle: the "N Spaces" tab, no teleport hint.
    const tab = await screen.findByRole("button", { name: /Spaces, .* active/ });
    expect(tab.parentElement).toHaveAttribute("data-teleport", "off");
    expect(screen.queryByRole("status", { name: /Teleport to Cua/ })).toBeNull();
    await waitFor(() => expect(drag.ready()).toBe(true));

    // A supported window-drag starts: the tab collapses and the notch grows into
    // the "Teleport to Cua" box (and the portal window grows to fit it).
    drag.emit({
      phase: "start",
      x: 400,
      y: 400,
      windowId: 7,
      appId: "chrome",
      appName: "Google Chrome",
      supported: true,
    });
    await waitFor(() => expect(bridge.modes).toContain("ambient-teleport"));
    expect(tab.parentElement).toHaveAttribute("data-teleport", "on");
    expect(screen.getByRole("status", { name: /Teleport to Cua/ })).toBeInTheDocument();

    // Bringing the window up to the notch (centre-x 756, top of screen) expands
    // the full Space selector as a drop surface.
    drag.emit({ phase: "move", x: 756, y: 0 });
    await screen.findByRole("listbox");
    await waitFor(() => expect(bridge.modes.at(-1)).toBe("switcher"));
    // The selector owns the screen now, with a concise drop hint in the strip;
    // the compact notch "Teleport to Cua" box is gone.
    expect(screen.getByText(/Drop on a Space/)).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: /Teleport to Cua/ })).toBeNull();
  });

  it("opens the selector only when pushed into the notch, never on a resize", async () => {
    const bridge = nativeNotchedBridge();
    const drag = fakeWindowDrag();
    let clock = FIXTURE_NOW;
    render(<App bridge={bridge} windowDrag={drag.bridge} now={() => (clock += 10)} />);
    const tab = await screen.findByRole("button", { name: /Spaces, .* active/ });
    await waitFor(() => expect(drag.ready()).toBe(true));

    // Chrome's left edge dragged: the origin moved, but so did the width.
    // Nothing shows, even at the notch.
    drag.emit({
      phase: "start",
      x: 380,
      y: 400,
      windowId: 7,
      appId: "chrome",
      appName: "Google Chrome",
      supported: true,
      startFrame: { x: 400, y: 300, width: 800, height: 600 },
      frame: { x: 380, y: 300, width: 820, height: 600 },
    });
    drag.emit({ phase: "move", x: 756, y: 0 });
    drag.emit({ phase: "end", x: 756, y: 0 });
    expect(bridge.modes).not.toContain("ambient-teleport");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(tab.parentElement).toHaveAttribute("data-teleport", "off");

    // A moved window brought to the top of the screen beside the notch, or
    // just under the notch's line: the box, but no selector.
    drag.emit({
      phase: "start",
      x: 400,
      y: 400,
      windowId: 7,
      appId: "chrome",
      appName: "Google Chrome",
      supported: true,
      startFrame: { x: 400, y: 300, width: 800, height: 600 },
      frame: { x: 440, y: 260, width: 800, height: 600 },
    });
    await waitFor(() => expect(bridge.modes).toContain("ambient-teleport"));
    drag.emit({ phase: "move", x: 300, y: 0 });
    drag.emit({ phase: "move", x: 756, y: 21 });
    expect(screen.queryByRole("listbox")).toBeNull();
    // Above the line: the selector.
    drag.emit({ phase: "move", x: 756, y: 19 });
    await screen.findByRole("listbox");
  });
});

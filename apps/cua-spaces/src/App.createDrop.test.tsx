// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { FIXTURE_NOW } from "./model/fixtures";
import type { SpaceRow } from "./model/spaces";
import type { NativeBridge } from "./native/bridge";
import type { FleetBridge } from "./native/fleet";
import type { TeleportBridge } from "./native/teleport";
import type { PortalEnvironment, PortalGeometry, WindowMode } from "./native/types";
import type { WindowDragBridge, WindowDragEvent } from "./native/windowDrag";
import { fakeFleetBridge } from "./test/fakeFleet";

// The native drag effect reaches for a couple of Tauri window APIs; stub them.
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
function nativeNotchedBridge(): NativeBridge {
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
    getEnvironment: async () => env(),
    setWindowMode: async (mode) => geom(mode),
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

const NEW_SPACE: SpaceRow = {
  id: "local:space-new",
  name: "space-new",
  provider: "local",
  spacesdVersion: "0.4.0",
  features: ["desktop_stream", "window_stream", "teleport.firefox"],
  os: "linux",
  reachable: true,
};

function fakeFleet(
  createSpace: FleetBridge["createSpace"],
  defaultLocation: FleetBridge["defaultLocation"] = async () => ({ value: "local", source: "config", path: "/tmp/cua/config.toml" }),
): FleetBridge {
  let created = false;
  return fakeFleetBridge({
    isNative: true,
    status: async () => ({
      configured: true,
      authMode: "static-token",
      baseUrl: "https://run.cua.ai",
      tokenUrl: "",
    }),
    listSpaces: async () => (created ? [NEW_SPACE] : []),
    defaultLocation,
    createSpace: async (config, pendingId) => {
      const row = await createSpace(config, pendingId);
      created = true;
      return row;
    },
  });
}

function fakeTeleport(openPicker: TeleportBridge["openPicker"]): TeleportBridge {
  const reject = () => Promise.reject(new Error("not used"));
  return {
    isNative: true,
    manifest: reject as never,
    push: reject as never,
    listOpenWindows: reject as never,
    captureThumbnail: async () => null,
    appIcon: async () => null,
  spaceAppIcon: async () => null,
  spacePrimaryDisplay: async () => null,
  spaceAppIcons: async (_s: string, requests: unknown[]) => requests.map(() => null),
    listRemoteWindows: reject as never,
    listSpaceAgents: async () => [],
    remoteWindowThumbnail: reject as never,
    streamRemoteWindow: reject as never,
    openPicker,
    pickerConfig: reject as never,
    closePicker: async () => {},
  };
}

afterEach(() => vi.restoreAllMocks());

describe("drag an app onto the + tile", () => {
  it("creates a Space in the default location and teleports the app into it", async () => {
    const createSpace = vi.fn(async () => NEW_SPACE);
    const defaultLocation = vi.fn(async () => ({ value: "local" as const, source: "config" as const, path: "/tmp/cua/config.toml" }));
    const openPicker = vi.fn(async () => {});
    const drag = fakeWindowDrag();
    let clock = FIXTURE_NOW;
    render(
      <App
        bridge={nativeNotchedBridge()}
        fleet={fakeFleet(createSpace, defaultLocation)}
        teleport={fakeTeleport(openPicker)}
        windowDrag={drag.bridge}
        now={() => (clock += 1000)}
      />,
    );

    await waitFor(() => expect(drag.ready()).toBe(true));
    await waitFor(() => expect(defaultLocation).toHaveBeenCalled());

    // Drag a supported app up to the notch → the Space selector expands.
    drag.emit({
      phase: "start",
      x: 400,
      y: 400,
      windowId: 7,
      appId: "chrome",
      appName: "Google Chrome",
      supported: true,
      capability: "full",
      entry: { id: "chrome", name: "Google Chrome", capability: "full" },
    });
    drag.emit({ phase: "move", x: 756, y: 0 });
    const newTile = await screen.findByRole("option", { name: /Drop here for a new Space/ });

    // The renderer hit-tests via elementFromPoint (absent in jsdom); land it on
    // the "+" tile. tileAt reads `.closest("[data-space-id]")`, which the tile has.
    document.elementFromPoint = (() => newTile) as typeof document.elementFromPoint;
    drag.emit({ phase: "end", x: 756, y: 10 });

    // The create names no location, so the shell uses the default location
    // (here This Mac) and the SDK's default image; then the dragged app
    // teleports into the new Space. It never assumes the cloud.
    await waitFor(() => expect(createSpace).toHaveBeenCalled());
    expect(createSpace).toHaveBeenCalledWith(undefined, expect.stringMatching(/^pending:/));
    await waitFor(() =>
      expect(openPicker).toHaveBeenCalledWith(
        expect.objectContaining({
          spaceId: "local:space-new",
          app: { id: "chrome", name: "Google Chrome" },
          // The SDK catalog entry travels with the drag, so the picker opens
          // straight on the app.
          entry: { id: "chrome", name: "Google Chrome", capability: "full" },
        }),
      ),
    );
  });

  it("does not create a Space when dropped between tiles (no target)", async () => {
    const createSpace = vi.fn(async () => NEW_SPACE);
    const openPicker = vi.fn(async () => {});
    const drag = fakeWindowDrag();
    let clock = FIXTURE_NOW;
    render(
      <App
        bridge={nativeNotchedBridge()}
        fleet={fakeFleet(createSpace)}
        teleport={fakeTeleport(openPicker)}
        windowDrag={drag.bridge}
        now={() => (clock += 1000)}
      />,
    );
    await waitFor(() => expect(drag.ready()).toBe(true));
    drag.emit({
      phase: "start",
      x: 400,
      y: 400,
      windowId: 7,
      appId: "chrome",
      appName: "Google Chrome",
      supported: true,
    });
    drag.emit({ phase: "move", x: 756, y: 0 });
    await screen.findByRole("listbox");
    // Drop over empty space (elementFromPoint → null).
    document.elementFromPoint = (() => null) as typeof document.elementFromPoint;
    drag.emit({ phase: "end", x: 756, y: 10 });
    // Give any pending microtasks a chance, then assert nothing was created.
    await act(async () => {
      await Promise.resolve();
    });
    expect(createSpace).not.toHaveBeenCalled();
    expect(openPicker).not.toHaveBeenCalled();
  });
});

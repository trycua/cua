// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Offscreen capture harness for the main window's Space detail (windows, agents).
 *
 * This renders the REAL `MainWindow` — the real `SpaceWindowList`, the
 * real `SpaceAgentList`, the real `TeleportDropZone`, the real stylesheet — and
 * feeds it data captured live from a Space rather than invented for the picture:
 *
 *   * `realRuns.json` is the output of `agents::parse_runs` (the same function
 *     `list_space_agents` returns from) over the actual record stream read out
 *     of cua-space-e3c1b54907, where three agents had been started through the
 *     agents MCP; and
 *   * `realWindows.json` is that Space's actual window list.
 *
 * Only the transport is stubbed. Nothing about the statuses, prompts or exit
 * codes in the figure was authored here: a run reads "failed" because that agent
 * really exited 127, and "running" because that process was really alive when
 * the records were read.
 *
 * Not part of the app bundle — `shot.html` is a separate entry, used by
 * scripts/shot-agents.sh.
 */
import { createRoot } from "react-dom/client";

import { MainWindow } from "../components/desktop/MainWindow";
import type { SpaceAgentRun } from "../model/agents";
import type { RemoteWindow } from "../model/teleport";
import type { TeleportBridge } from "../native/teleport";
import { fakeFleetBridge } from "../test/fakeFleet";
import "../styles/tokens.css";
import "../styles/app.css";
import "../styles/desktop.css";

import realAppIcons from "./realAppIcons.json";
import realRuns from "./realRuns.json";
import realWindows from "./realWindows.json";

const SPACE_ID = "local:cua-space-e3c1b54907";

interface RawWindow {
  window: string;
  target_epoch: number;
  app_name: string;
  title: string;
  visible?: boolean;
  geometry?: { width_px?: number; height_px?: number };
}

/**
 * The same windows the real app would show. `parse_remote_windows` in the shell
 * drops what is not an app window — guest desktop chrome, the Space's own
 * capture stack, macOS permission prompts — and the raw rcdp list this fixture
 * was captured from is pre-filter, so the filter is mirrored here. Keep this in
 * step with `is_remote_desktop_chrome` in src-tauri/src/teleport.rs.
 */
const NOT_AN_APP_WINDOW = [
  "xfdesktop",
  "xfce4-panel",
  "xfwm4",
  "xfce4-notifyd",
  "rcdp host",
  "rcdphost",
  "universalaccessauthwarn",
];

const windows: RemoteWindow[] = (realWindows as RawWindow[])
  .filter((raw) => {
    const name = raw.app_name.trim().toLowerCase();
    return !NOT_AN_APP_WINDOW.some((skip) => name.includes(skip));
  })
  .map((raw) => ({
    id: raw.window,
    appId: raw.app_name,
    appName: raw.app_name,
    title: raw.title,
    visible: raw.visible ?? true,
    targetEpoch: raw.target_epoch,
    widthPx: raw.geometry?.width_px,
    heightPx: raw.geometry?.height_px,
  }));

const teleport = {
  isNative: true,
  listOpenWindows: async () => [],
  captureThumbnail: async () => null,
  appIcon: async () => null,
  // The REAL icons, fetched from the guest by the same lookup space_app_icon
  // uses (NSWorkspace.fullPathForApplication -> iconForFile, via JXA) and
  // inlined as data URIs. Without a Tauri bridge this render would otherwise hit
  // the monogram fallback for every row, which is what the panel shows when an
  // icon CANNOT be resolved -- a figure full of fallbacks says the product is
  // broken. Returning null for an app not in the map keeps that fallback honest.
  spaceAppIcon: async (_space: string, appName: string) =>
    (realAppIcons as Record<string, string>)[appName] ?? null,
  spacePrimaryDisplay: async () => null,
  spaceAppIcons: async (_s: string, requests: { appName: string }[]) =>
    requests.map((r) => (realAppIcons as Record<string, string>)[r.appName] ?? null),
  listRemoteWindows: async () => windows,
  listSpaceAgents: async () => realRuns as SpaceAgentRun[],
  remoteWindowThumbnail: async () => null,
  streamRemoteWindow: async () => {},
  listSpaces: async () => [],
  closePicker: async () => {},
  manifest: async () => {
    throw new Error("not used in the capture");
  },
  push: async () => {
    throw new Error("not used in the capture");
  },
} as unknown as TeleportBridge;

const fleet = fakeFleetBridge({
  isNative: true,
  listSpaces: async () => [
    {
      id: SPACE_ID,
      name: "cua-space-e3c1b54907",
      provider: "local",
      spacesdVersion: "0.4.0",
      features: ["desktop_stream", "window_stream"],
      os: "macos",
      reachable: true,
    },
    { id: "cloud:aurora", name: "aurora", provider: "cloud", spacesdVersion: "0.4.0", features: [], os: "linux", reachable: true },
    { id: "cloud:orion", name: "orion", provider: "cloud", spacesdVersion: "0.4.0", features: [], os: "linux", reachable: false },
  ],
  status: async () => ({ configured: true, authMode: "user", baseUrl: "", tokenUrl: "", identity: "ada@example.com" }),
});

const host = document.getElementById("root");
if (host) {
  createRoot(host).render(<MainWindow fleet={fleet} teleport={teleport} />);
  // Open the Space the way a user does, by clicking its row.
  const open = () => {
    const row = Array.from(host.querySelectorAll<HTMLButtonElement>('[role="option"]')).find((el) =>
      el.textContent?.includes("Cua Space E3c1b54907"),
    );
    if (!row) {
      window.setTimeout(open, 50);
      return;
    }
    row.click();
    window.setTimeout(() => {
      document.body.dataset.shotReady = "true";
    }, 400);
  };
  window.setTimeout(open, 120);
}

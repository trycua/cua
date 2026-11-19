// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceAgentRun } from "../model/agents";
import type { SpaceUsage } from "../model/window";
import type {
  TeleportPickerConfig,
  TeleportResult,
  LocalApp,
  OpenWindow,
  RemoteWindow,
  TransferManifest,
} from "../model/teleport";
import { hasTauri } from "./bridge";

/**
 * Typed wrapper over the shell's teleport/teleport commands (cua SDK). Outside Tauri the
 * fallback serves a synthetic manifest (clearly labelled) so the consent flow
 * can be designed and tested with no CLI and no network; `push` never touches
 * a network in the fallback.
 */
export interface TeleportBridge {
  readonly isNative: boolean;
  /** The host-side teleport manifest for an app (read-only; cua SDK providers). */
  manifest(appId: string, scope: string): Promise<TransferManifest>;
  /**
   * Teleport the checked items into the Space (cua SDK `Space::teleport`, an
   * `Approval` minted from exactly `include`). `include` is the `rel_path` of
   * every checked consent item; `acknowledgeSensitive` must be true iff that
   * confirmed selection contains a sensitive item (the SDK refuses otherwise).
   * Only call after explicit user consent.
   */
  push(
    appId: string,
    scope: string,
    spaceId: string,
    include: string[],
    acknowledgeSensitive: boolean,
  ): Promise<TeleportResult>;
  /**
   * The user's currently-open desktop windows, for the "Teleport" picker grid
   * (macOS `CGWindowListCopyWindowInfo`). Empty outside the native shell.
   */
  listOpenWindows(): Promise<OpenWindow[]>;
  /**
   * A `data:image/png;base64,…` preview of one window, or null when it could
   * not be captured (Screen Recording not granted — the card falls back to an
   * app tile). Null outside the native shell.
   */
  captureThumbnail(windowId: number): Promise<string | null>;
  /**
   * The owning app's icon as a `data:image/png;base64,…` URL, resolved lazily
   * (keyed by the `appId` from `listOpenWindows`) so the picker grid renders
   * instantly and streams icons in per-app. Null when it can't be resolved or
   * outside the native shell.
   */
  appIcon(appId: string): Promise<string | null>;
  /**
   * The icon of an app running INSIDE a Space, as a `data:image/png;base64,…`
   * URL. `appIcon` only knows apps running on THIS Mac, so guest apps the user
   * does not also run locally resolved to nothing and the rows fell back to a
   * monogram; for a Local Space the guest itself is asked. Null only when the
   * app is genuinely unidentifiable (the shell logs why).
   */
  spaceAppIcon(spaceId: string, appName: string, appId: string, pid?: number): Promise<string | null>;
  /**
   * Icons for many windows' apps in one call (the SDK's `Space.app_icons`:
   * its one icon cache, every miss in one guest round trip), in request
   * order; null where the Space has none. Keep no icon cache in the UI.
   */
  spaceAppIcons(
    spaceId: string,
    requests: { appName: string; appId: string; pid: number }[],
  ): Promise<(string | null)[]>;
  /**
   * The Space's primary display size (its display list), for the Stream
   * section's "Desktop (W×H)" row. Null when unknown or outside the shell.
   */
  spacePrimaryDisplay(spaceId: string): Promise<{ widthPx: number; heightPx: number } | null>;
  /** The Space's memory and storage use now (the SDK's `Space.usage`). */
  spaceUsage?(spaceId: string): Promise<SpaceUsage | null>;
  /**
   * The Space's REMOTE windows (the picker's "This Mac" tab), enumerated by
   * the spacesd's `StreamService.ListTargets`. Rejects when the
   * Space isn't reachable; empty outside the native shell.
   */
  listRemoteWindows(spaceId: string): Promise<RemoteWindow[]>;
  /**
   * The coding-agent runs inside a Space, read from the records `agent_start`
   * writes there. REJECTS when the Space could not be asked — the AGENTS
   * section shows that as "couldn't read this Space's agents", because an empty
   * list would read as "no agents are running", which is a different and
   * possibly false claim. Empty outside the native shell.
   */
  listSpaceAgents(spaceId: string): Promise<SpaceAgentRun[]>;
  /**
   * A `data:image/png;base64,…` frame-capture thumbnail of one of the Space's
   * remote windows (a spacesd window screenshot), or
   * null when it could not be captured (host unreachable, no frame). Null
   * outside the native shell.
   */
  remoteWindowThumbnail(
    spaceId: string,
    windowId: string,
    targetEpoch: number,
  ): Promise<string | null>;
  /**
   * Stream ONE of the Space's remote windows onto this Mac (the "This Mac"
   * tab's action): opens a dedicated `winone-<space>-<window>` OS window sized
   * to that remote window, with the OS frame as the window chrome. `appName`
   * and `title` seed the OS window title. No-op outside the native shell.
   *
   * `replica` is the shift-click testing affordance: instead of focusing the
   * window that is already streaming this target, open an ADDITIONAL one. The
   * two windows are separate webviews, so each opens its own media session and
   * joins as its own participant — which is what lets one person drive stream 1
   * and watch their cursor appear in stream 2. Capped at two per target.
   */
  streamRemoteWindow(
    spaceId: string,
    spaceName: string,
    windowId: string,
    appName: string,
    title: string,
    replica?: boolean,
  ): Promise<void>;
  /**
   * Open (or re-target) the centered picker window for a Space. `app` pre-selects
   * an app for the drag/`.app`-drop shortcut. No-op outside the native shell.
   */
  openPicker(request: {
    spaceId: string;
    spaceName: string;
    app?: LocalApp | null;
    /** The app's SDK catalog entry (core JSON), from a drop or window drag. */
    entry?: Record<string, unknown> | null;
    /** Files or folders dropped with the app. */
    files?: string[];
  }): Promise<void>;
  /**
   * The Stream section rows whose picture-in-picture panel is open now (the
   * core's row ids: "desktop" for the Space's mirror, a window's handle for
   * its stream windows).
   */
  streamPanels?(spaceId: string): Promise<string[]>;
  /** Closes every stream window of one of the Space's windows. */
  closeStreamWindow?(spaceId: string, windowId: string): Promise<void>;
  /** Calls `listener` whenever a Stream panel opens or closes; returns the
   * unsubscribe. */
  onStreamPanelsChanged?(listener: () => void): Promise<() => void>;
  /** Read the centered picker window's target (only valid on that window). */
  pickerConfig(): Promise<TeleportPickerConfig>;
  /** Close the centered picker window. No-op outside the native shell. */
  closePicker(): Promise<void>;
}

function fallbackManifest(appId: string, scope: string): TransferManifest {
  const full = scope === "full";
  return {
    provider_id: appId,
    app_display_name: appId === "google-chrome" ? "Google Chrome" : appId,
    scope: full ? "full_profile" : "tabs_only",
    items: [
      { label: "Open tabs", rel_path: "tabs.json", est_bytes: 24_576, count: 18, count_noun: "tabs", sensitive: false },
      ...(full
        ? [
            { label: "Bookmarks", rel_path: "Default/Bookmarks", est_bytes: 45_000, count: 42, count_noun: "bookmarks", sensitive: false },
            { label: "Preferences", rel_path: "Default/Preferences", est_bytes: 8_192, sensitive: false },
            { label: "Cookies", rel_path: "Default/Cookies", est_bytes: 486_400, count: 311, count_noun: "cookies", sensitive: true },
            { label: "Login Data", rel_path: "Default/Login Data", est_bytes: 65_536, count: 12, count_noun: "logins", sensitive: true },
            { label: "History", rel_path: "Default/History", est_bytes: 262_144, count: 4_920, count_noun: "history entries", sensitive: true },
          ]
        : []),
    ],
    total_est_bytes: full ? 846_848 : 24_576,
    notes: ["Simulated manifest — browser preview, nothing is read or sent."],
    // Browsers can route through the hotspot; preview it so the option shows.
    supports_hotspot: appId === "google-chrome",
  };
}

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export function createFallbackTeleportBridge(): TeleportBridge {
  return {
    isNative: false,
    manifest: async (appId, scope) => {
      await delay(250);
      return fallbackManifest(appId, scope);
    },
    push: async (appId, _scope, _spaceId, _include, _ack) => {
      await delay(600);
      return { ok: true, provider_id: appId, launched: true, pid: null };
    },
    // Browser/test builds have no window list; the picker shows its empty state.
    listOpenWindows: async () => [],
    captureThumbnail: async () => null,
    appIcon: async () => null,
    spaceAppIcon: async () => null,
    spaceAppIcons: async (_spaceId, requests) => requests.map(() => null),
    spacePrimaryDisplay: async () => null,
    // No Space outside the native shell; the "This Mac" tab shows its empty state.
    listRemoteWindows: async () => [],
    listSpaceAgents: async () => [],
    remoteWindowThumbnail: async () => null,
    streamRemoteWindow: async () => {},
    openPicker: async () => {},
    pickerConfig: async () => {
      throw new Error("the teleport picker window needs the Tauri shell");
    },
    closePicker: async () => {},
  };
}

export function createTauriTeleportBridge(): TeleportBridge {
  // Imported lazily so the browser/test bundle never touches Tauri globals.
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    manifest: (appId, scope) => invoke<TransferManifest>("teleport_manifest", { appId, scope }),
    push: (appId, scope, spaceId, include, acknowledgeSensitive) =>
      invoke<TeleportResult>("teleport_push", {
        appId,
        scope,
        spaceId,
        include,
        acknowledgeSensitive,
      }),
    listOpenWindows: () => invoke<OpenWindow[]>("list_open_windows"),
    captureThumbnail: (windowId) =>
      invoke<string | null>("capture_window_thumbnail", { windowId }),
    appIcon: (appId) => invoke<string | null>("app_icon", { appId }),
    spaceAppIcon: (spaceId, appName, appId, pid) =>
      invoke<string | null>("space_app_icon", { spaceId, appName, appId, pid: pid ?? null }),
    spaceUsage: (spaceId) => invoke<SpaceUsage | null>("space_usage", { spaceId }),
    spaceAppIcons: (spaceId, requests) => invoke<(string | null)[]>("space_app_icons", { spaceId, requests }),
    spacePrimaryDisplay: (spaceId) =>
      invoke<{ widthPx: number; heightPx: number } | null>("space_primary_display", { spaceId }),
    listRemoteWindows: (spaceId) => invoke<RemoteWindow[]>("list_remote_windows", { spaceId }),
    listSpaceAgents: (spaceId) => invoke<SpaceAgentRun[]>("list_space_agents", { spaceId }),
    remoteWindowThumbnail: (spaceId, windowId, targetEpoch) =>
      invoke<string | null>("remote_window_thumbnail", { spaceId, windowId, targetEpoch }),
    streamRemoteWindow: (spaceId, spaceName, windowId, appName, title, replica = false) =>
      invoke<void>("stream_remote_windows", {
        spaceId,
        spaceName,
        windowId,
        appName,
        title,
        replica,
      }),
    streamPanels: (spaceId) => invoke<string[]>("stream_panels", { spaceId }),
    closeStreamWindow: (spaceId, windowId) => invoke<void>("close_stream_window", { spaceId, windowId }),
    onStreamPanelsChanged: async (listener) => {
      const { listen } = await import("@tauri-apps/api/event");
      return listen("stream-panels:changed", () => listener());
    },
    openPicker: (request) => invoke<void>("open_teleport_picker", { request }),
    pickerConfig: () => invoke<TeleportPickerConfig>("teleport_picker_config"),
    closePicker: () => invoke<void>("close_teleport_picker"),
  };
}

export function createTeleportBridge(): TeleportBridge {
  return hasTauri() ? createTauriTeleportBridge() : createFallbackTeleportBridge();
}

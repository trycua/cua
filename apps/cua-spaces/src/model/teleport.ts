// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";

/**
 * Teleport picker types: the session-teleport wire types, the picker
 * window's config, and the window lists of its "Open windows" and "From
 * {Space}" tabs. The decisions are the app core's (`teleport::windows`). The "Teleport an app…" flow itself (catalog, plan,
 * consent, run) is the cua SDK's headless picker (`@trycua/cua/teleport`).
 */

/** rcdp `ManifestItem` JSON, snake_case preserved from the CLI. */
export interface ManifestItem {
  label: string;
  rel_path: string;
  est_bytes: number;
  /**
   * Count of concrete things this item holds (tabs, bookmarks, cookies,
   * logins, history entries), when the exporter could determine it cheaply.
   * The consent checklist prefers "{count} {count_noun}" over the byte size;
   * absent when it couldn't be counted (locked DB, directory) — size is shown.
   */
  count?: number;
  /** Plural noun for `count` ("bookmarks", "tabs"); absent when `count` is. */
  count_noun?: string;
  sensitive: boolean;
  /**
   * Whether the provider wants this item checked by default in the consent
   * sheet. Absent for older providers that don't declare it (the checklist then
   * falls back to the "tabs" heuristic).
   */
  default_checked?: boolean;
}

/** rcdp `TransferManifest` JSON. */
export interface TransferManifest {
  provider_id: string;
  app_display_name: string;
  scope: "tabs_only" | "full_profile";
  items: ManifestItem[];
  total_est_bytes: number;
  notes: string[];
  /** The app declares it can be auto-configured to route through a network
   * hotspot (browser honoring a SOCKS proxy). Surfaces the "Network
   * Fingerprint" option in the picker. Passthrough from the rcdp manifest. */
  supports_hotspot?: boolean;
}

/** The teleport receiver's import response, relayed by `rcdp teleport push`. */
export interface TeleportResult {
  ok: boolean;
  provider_id: string;
  launched: boolean;
  /** Always null: the SDK's TeleportService import does not report a pid. */
  pid: number | null;
  /** Entries the Space imported / skipped, when the receiver reported them. */
  imported?: string[];
  skipped?: string[];
}

export interface LocalApp {
  id: string;
  name: string;
}

/** One currently-open desktop window, offered in the teleport picker grid. */
export interface OpenWindow {
  /** CoreGraphics window number, used to capture a preview thumbnail. */
  windowId: number;
  appId: string;
  appName: string;
  windowTitle: string;
  /** Whether teleport can bring the app up in a Space (full or install only). */
  supported: boolean;
  /** The SDK's capability level for the app. */
  capability?: "full" | "install_only" | "unsupported";
  /** The owning app's bundle, when known. */
  bundlePath?: string | null;
  /** The app's catalog entry (the SDK's core JSON), for a preselected picker. */
  entry?: Record<string, unknown> | null;
  /**
   * The owning app's icon as a `data:image/png;base64,…` URL (from the shell's
   * `NSRunningApplication(pid).icon`), or null when it could not be resolved —
   * the card then shows a monogram fallback. Always null outside the native shell.
   */
  icon: string | null;
}

/**
 * One of the Space's REMOTE windows, enumerated through the rcdp CLI
 * (`rcdp targets list`) for the picker's "This Mac" tab. The card shows the app
 * icon (resolved from `appId`) and a frame-capture thumbnail fetched via
 * `remoteWindowThumbnail(spaceId, id, targetEpoch)`.
 */
export interface RemoteWindow {
  /** Stable key (the rcdp window handle, stringified) for React + selection. */
  id: string;
  appName: string;
  title: string;
  visible: boolean;
  /** rcdp provider/app id for icon lookup ("Google Chrome" -> "google-chrome"). */
  appId: string;
  /** The window handle's epoch, passed to `session open --epoch` on capture. */
  targetEpoch: number;
  /** Pixel geometry rcdp reported, when it reported any. */
  widthPx?: number;
  heightPx?: number;
  /** Owning process (the SDK's app icon lookup), when reported. */
  pid?: number;
}

/** One app with open windows, for the picker's app-filter row. */
export interface OpenApp {
  appId: string;
  appName: string;
  supported: boolean;
}

/** The centered picker's target (read from the shell on the picker window). */
export interface TeleportPickerConfig {
  spaceId: string;
  spaceName: string;
  /** Legacy pre-selection by id and name (kept for callers without an entry). */
  app: LocalApp | null;
  /**
   * Pre-selected app as the SDK catalog's core JSON (a dropped bundle or a
   * dragged window); the picker opens straight on its options.
   */
  entry?: Record<string, unknown> | null;
  /** Files or folders dropped with the app. */
  files?: string[];
  /** Go straight to the consent screen (captures and demos only). */
  autoReview?: boolean;
}

/** An open window as the core sees it (the shell-only fields stay here). */
function coreWindow(win: OpenWindow) {
  return {
    windowId: win.windowId,
    appId: win.appId,
    appName: win.appName,
    windowTitle: win.windowTitle,
    supported: win.supported,
  };
}

/**
 * Filter open windows by a free-text query, matching either the app name or
 * the window title, case-insensitively. Blank query returns everything. Pure
 * so the picker's live filter is unit-testable.
 */
export function filterWindows(windows: readonly OpenWindow[], query: string): OpenWindow[] {
  const kept = core<{ windowId: number }[]>("windows.filterWindows", { windows: windows.map(coreWindow), query });
  const ids = new Set(kept.map((w) => w.windowId));
  return windows.filter((w) => ids.has(w.windowId));
}

/** Distinct apps (first-seen order) across a window list, for the app row. */
export function appsFromWindows(windows: readonly OpenWindow[]): OpenApp[] {
  return core("windows.appsFromWindows", { windows: windows.map(coreWindow) });
}

/**
 * The "Teleport" button is live only when a window is selected AND its app has
 * a working provider; unsupported selections keep the button disabled (honest
 * about what rcdp can currently move).
 */
export function teleportButtonEnabled(selected: OpenWindow | null): boolean {
  return core("windows.teleportButtonEnabled", { selected: selected ? coreWindow(selected) : null });
}

/**
 * Which side of the picker is active:
 *  - "space": the user's LOCAL Mac windows, teleported INTO the Space.
 *  - "thisMac": the Space's REMOTE windows, streamed onto this Mac.
 */
export type PickerTab = "space" | "thisMac";

/**
 * What the primary picker button should be, given the active tab and the
 * selected item. Pure so the label/action wiring is unit-testable.
 *
 *  - This Mac tab → always "Stream" (stream the picked remote window here).
 *  - Space tab, supported app → "Teleport" (the existing consent → push flow).
 *  - Space tab, UNSUPPORTED app → "Stream", but the local→remote window host
 *    doesn't exist yet, so the click only surfaces an honest "coming soon"
 *    notice (`action: "stream-local-soon"`) and does nothing destructive.
 */
export interface PickerPrimary {
  label: "Teleport" | "Stream";
  action: "teleport" | "stream-remote" | "stream-local-soon";
  enabled: boolean;
}

export function pickerPrimary(
  tab: PickerTab,
  selected: { supported: boolean } | null,
): PickerPrimary {
  return core("windows.pickerPrimary", { tab, selectedSupported: selected ? selected.supported : null });
}

/** Filter the Space's remote windows by a free-text query (app or title). */
export function filterRemoteWindows(
  windows: readonly RemoteWindow[],
  query: string,
): RemoteWindow[] {
  const kept = core<RemoteWindow[]>("windows.filterRemoteWindows", { windows, query });
  const ids = new Set(kept.map((w) => w.id));
  return windows.filter((w) => ids.has(w.id));
}

/** One app's windows, for the app-grouped window list (Screens/Windows). */
export interface RemoteWindowGroup {
  appId: string;
  appName: string;
  windows: RemoteWindow[];
}

/**
 * Group a Space's remote windows by owning app, preserving the order the
 * windows arrived in (so the first-seen app leads and each app's windows keep
 * their z-order). Mirrors the reference picker's "Windows" section, where an
 * app row heads its own windows.
 */
export function groupWindowsByApp(windows: readonly RemoteWindow[]): RemoteWindowGroup[] {
  const byId = new Map(windows.map((w) => [w.id, w]));
  return core<RemoteWindowGroup[]>("windows.groupWindowsByApp", { windows }).map((g) => ({
    ...g,
    windows: g.windows.map((w) => byId.get(w.id) ?? w),
  }));
}

/**
 * Whether a remote target is the guest's whole screen rather than an app
 * window. rcdp lists the driver's own capture target alongside real windows;
 * it carries the screen's geometry and no title, so it becomes the "Screens"
 * row instead of an app nobody can stream meaningfully.
 */
export function isScreenTarget(win: RemoteWindow): boolean {
  return core("windows.isScreenTarget", { window: win });
}

/** Label for the Screens row: "Desktop (1024×768)" when the size is known. */
export function screenLabel(screen: RemoteWindow | null | undefined): string {
  return core("windows.screenLabel", { screen: screen ?? null });
}

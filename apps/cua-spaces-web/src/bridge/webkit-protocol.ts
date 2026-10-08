// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The contract between the web UI and the native hosts: the SwiftUI app's
 * WKWebView (apps/cua-spaces-macos, `WebHost/WebUIBridge.swift`) and the
 * Electron shell's main process (apps/cua-spaces-desktop, `src/bridge/`),
 * which answers the same methods with the same shapes over its own
 * transport (`electron-channels.ts`, `transport.ts`).
 *
 * Page -> host: `window.webkit.messageHandlers.cua.postMessage(request)`.
 * The handler is a `WKScriptMessageHandlerWithReply`, so `postMessage`
 * returns a Promise that resolves with the response:
 *
 *     { id, method, args? }  ->  { id, ok: true, result } | { id, ok: false, error: { code, message } }
 *
 * `code`: `unimplemented`, `bad_args`, `not_found`, `cancelled`,
 * `native_only`, `forbidden` or `failed`. A failed host setup or This
 * machine button also says `title`, `details` (the raw error) and
 * `actionLabel` (`HostSetupFailure`), and `message` is then the plain words.
 *
 * Host -> page events: `window.dispatchEvent(new CustomEvent("cua:event",
 * { detail: { event, payload } }))`, where `event` is one of `WEBKIT_EVENTS`.
 * The page asks again for what it shows.
 *
 * Results are the app core's records as the SwiftUI views get them
 * (`BridgeValue.swift`): camelCase fields, enums as their case name, or
 * `{ type, ...fields }` with a payload. `adapters/webkit.ts` maps them onto
 * `HostOperations` (`protocol.ts`).
 */

import type { MachineAccessNotice } from "./contracts/devices";
import { AGENT_KEYS_WEBKIT_METHODS } from "./ops/agent-keys";
import { KEYVAULT_MANAGE_WEBKIT_METHODS } from "./ops/keyvault-manage";
import { KEYVAULT_SETUP_WEBKIT_METHODS } from "./ops/keyvault-setup";

export const WEBKIT_HANDLER_NAME = "cua";
/** The window event the host dispatches. */
export const WEBKIT_EVENT = "cua:event";

/** Every method the SwiftUI host routes (`WebUIBridge.methods`).
 * `spaces.cancelCreate`, `host.status`, `keyvault.approve`, `keyvault.deny`,
 * `keyvault.revokeGrant` and the `agents.*` methods have the operation's own
 * name and arguments (`HostOperations`), like the Electron router's
 * `cua:<op>` channels. */
export const WEBKIT_METHODS = [
  "app.info",
  "session.get",
  "session.signIn",
  "session.signOut",
  "spaces.list",
  "spaces.open",
  "spaces.createOptions",
  "spaces.create",
  "spaces.setPower",
  "spaces.delete",
  "spaces.cancelCreate",
  "machines.list",
  "host.status",
  "agents.list",
  "agents.runs",
  "agents.events",
  "agents.pause",
  "agents.resume",
  "agents.setup",
  "agents.configure",
  ...AGENT_KEYS_WEBKIT_METHODS,
  ...KEYVAULT_SETUP_WEBKIT_METHODS,
  ...KEYVAULT_MANAGE_WEBKIT_METHODS,
  // The pages beyond Spaces, the Keyvault and Agents (ops/webkit-pages.ts).
  "spaces.add",
  "clouds.status",
  "clouds.test",
  "clouds.connect",
  "teleport.catalog",
  "teleport.entryForPath",
  "teleport.windows",
  "teleport.remoteWindows",
  "teleport.icon",
  "teleport.thumbnail",
  "teleport.plan",
  "teleport.run",
  "teleport.sites",
  "teleport.remembered",
  "teleport.streamWindow",
  "sharing.list",
  "sharing.share",
  "sharing.unshare",
  "volume.overview",
  "volume.storage",
  "volume.storageSet",
  "volume.mount",
  "volume.unmount",
  "volume.approve",
  "volume.deny",
  "volume.revoke",
  "volume.resolve",
  "volume.reveal",
  "agents.setupDriver",
  "about.get",
  "about.set",
  "about.checkNow",
  "loginItem.get",
  "loginItem.set",
  "loginItem.openSettings",
  "devices.get",
  "devices.enroll",
  "devices.checkEnrolled",
  "devices.approve",
  "devices.rename",
  "devices.revoke",
  "devices.confirmMachine",
  "storage.get",
  "storage.run",
  "notifications.list",
  "notifications.markAllRead",
  "telemetry.track",
  "spaces.usage",
  "spaces.windows",
  "stream.pip",
  "spaces.thumbnail",
  "spaces.chooseFiles",
  "spaces.droppedFiles",
  "spaces.sendFiles",
  "host.setUp",
  "host.action",
  "host.openSettings",
  "settings.get",
  "settings.choose",
  "keyvault.get",
  "keyvault.lock",
  "keyvault.unlock",
  "keyvault.unlockVault",
  "keyvault.setDisabled",
  "keyvault.approve",
  "keyvault.deny",
  "keyvault.revokeGrant",
  "window.setBackgroundColor",
  "window.setDragRegions",
  "startup.get",
  "startup.act",
] as const;

export type WebkitMethod = (typeof WEBKIT_METHODS)[number];

/** What only the Electron host answers: it draws a Space's video in the
 * page (WebCodecs), so the page asks it for a media ticket
 * (`ops/stream.ts`), and its first run is this page's (`onboarding.get`,
 * `onboarding.complete`); the Mac app draws video and its first run
 * natively. */
export const ELECTRON_HOST_METHODS = ["onboarding.get", "onboarding.complete", "spaces.openStream"] as const;

/** `onboarding.get` and `onboarding.complete`'s answer: whether the first run finished, and as what. */
export interface WkOnboarding {
  completed: boolean;
  mode: "client" | "host";
}

export type HostMethod = WebkitMethod | (typeof ELECTRON_HOST_METHODS)[number];

/** The app menu's Settings command (⌘,) while the SwiftUI app's New UI window is in front: go to Settings. */
export const WEBKIT_OPEN_SETTINGS_EVENT = "settings.openRequested";

export const WEBKIT_EVENTS = [
  "spaces.changed",
  "machines.changed",
  "keyvault.changed",
  "session.changed",
  "settings.changed",
  "agents.changed",
  "teleport.progress",
  "startup.changed",
] as const;
export type WebkitEventName = (typeof WEBKIT_EVENTS)[number];

export interface WebkitRequest {
  /** Unique per page load; the host echoes it. */
  id: string;
  method: HostMethod;
  args?: Record<string, unknown>;
}

export type WebkitResponse =
  | { id: string; ok: true; result: unknown }
  | { id: string; ok: false; error: { code: string; message: string; title?: string; details?: string; actionLabel?: string } };

/** `CustomEvent("cua:event").detail`. */
export interface WebkitEventDetail {
  event: WebkitEventName | (string & {});
  payload?: unknown;
}

export function isWebkitResponse(m: unknown): m is WebkitResponse {
  return typeof m === "object" && m !== null && typeof (m as { ok?: unknown }).ok === "boolean";
}

/* ---- What the host's read methods return (the parts the bridge reads) ---- */

/** `model::Space` as Swift encodes it (`AppSpace`). */
export interface WkSpace {
  id: string;
  name: string;
  os: "macos" | "windows" | "linux" | "unknown";
  status: "local" | "running" | "approval" | "suspended" | "provisioning" | "deleting";
  detail: string;
  lastUsedAt: number;
  startedAt?: number | null;
  provider?: "cloud" | "local" | "direct" | "relay" | null;
  sdk?: { features: string[]; spacesdVersion: string; reachable: boolean; error?: string | null } | null;
  osName?: string | null;
  osPrettyName?: string | null;
  image?: string | null;
  imageDigest?: string | null;
  kind?: "container" | "vm" | null;
  arch?: string | null;
  host?: string | null;
  hostName?: string | null;
  power?: { control: "suspend" | "stop" | (string & {}); off: boolean; turningOn?: boolean | null; error?: string | null } | null;
  cloud?: string | null;
  cloudPlace?: string | null;
  cloudDelete?: string | null;
  /** A create the app runs (its own pending row): its progress, or why it failed. */
  progress?: import("./contracts/spaces").SpaceProgress | null;
}

/** `spaces.list`, `spaces.refresh`, `spaces.select`, `spaces.setPower`, `spaces.delete`. */
export interface WkSpaces {
  loaded: boolean;
  selectedId: string | null;
  spaces: { space: WkSpace; deleting: boolean }[];
  /** The last registry read failed (`AppModel.rosterError`): the rows are
   * the ones listed before, and this says so. Null when it worked. */
  rosterError?: string | null;
}

/** `machines.list`: the sidebar and the account's devices. */
export interface WkMachines {
  /** The sidebar's "This machine" row (`spaces::sidebar::SidebarRow`):
   * `detail` is the host summary, `status` is `local` while it is set up
   * and sharing. */
  thisMachine?: { status: string; statusText: string; detail: string } | null;
  /** The account's devices (`devices::DevicesView`, the parts read here). */
  devices: {
    rows: { id: string; name: string; platform: string; current: boolean; detail?: string; lastSeen?: number | null }[];
  } | null;
  signedIn: boolean;
  /** This Mac's host status, as `host.status` answers it (null until the
   * host answered). The Machines page shows the core's "This machine" panel. */
  host?: WkHostState | null;
  /** What each machine on the relay reported as its hostname, by machine
   * id (to list a machine and its enrolled device once). */
  hostnames?: Record<string, string> | null;
  /** Whether the relay sees each of the account's machines connected now,
   * by machine id (the devices snapshot's machines). */
  presence?: Record<string, boolean> | null;
  /** Each device's state (`enrolled`, `pending`, `expired`, `revoked`), by id. */
  deviceStates?: Record<string, string> | null;
  /** Signed in, but this Mac cannot open the account's machines (never
   * enrolled, waiting, expired, revoked): `DevicesModel.accessNotice`. */
  accessNotice?: MachineAccessNotice | null;
}

/** `session.get`, `session.signIn`, `session.signOut`. */
export interface WkSession {
  identity: string | null;
  signedIn: boolean;
  cloudConfigured: boolean;
  /** `idle`, `starting`, `{type: "waiting", userCode}`, `{type: "failed", message}`. */
  signIn: string | { type: string; userCode?: string | null; message?: string };
}

export interface WkSettingsRow {
  id: string;
  kind: string;
  label: string;
  enabled: boolean;
  help?: string | null;
  options: { id: string; label: string; active: boolean }[];
}

/** `settings.get`, `settings.choose`, `settings.press`: the core's Settings page. */
export interface WkSettings {
  page: { title: string; sections: { id: string; rows: WkSettingsRow[] }[] };
  /** `AppSettings.updateChannel` (`stable` or `beta`); null while the build
   * has no updater. Set with `settings.choose {row: "update-channel"}`. */
  updateChannel?: string | null;
}

/** `keyvault.*`: the core's page views, plus the overview (camelCase records). */
export interface WkKeyvault {
  availability: string;
  overview?: Record<string, unknown> | null;
  /** Import ids of the copies hidden from the notch. */
  dismissed?: string[];
  busy: boolean;
  error: string | null;
}

/** `host.status`: `HostModel.state` (the core's flattened `HostStatus`) plus
 * the relay machine id; null until the host answered. */
export interface WkHostState {
  configured: boolean;
  mode?: string | null;
  relayUrl?: string | null;
  directUrl?: string | null;
  machineId?: string | null;
  name?: string | null;
  sharing: boolean;
  serviceInstalled: boolean;
  serviceRunning: boolean;
  serviceKind: string;
  online?: boolean | null;
  clients: { id: string; email?: string | null; name?: string | null; streams?: number | null }[];
  permissions: { id: string; title: string; settingsUrl?: string | null; instructions?: string | null; granted: boolean }[];
  error?: string | null;
  shareDesktop: boolean;
  provideSpaces: boolean;
  maxSpaces: number;
  maxMacosVms?: number | null;
  recentAccess?: { atMs: number; via: string; who: string; what: string }[] | null;
  accessLogError?: string | null;
  providedSpaces?: import("./contracts/host").HostProvidedSpace[] | null;
  spacesAudit?: import("./contracts/host").HostSpacesAudit[] | null;
  spacesAuditError?: string | null;
  pausedSignedOut?: boolean | null;
  owner?: string | null;
  ownerEmail?: string | null;
  /** Who is signed in, as `HostModel` last checked it. */
  account?: { id?: string | null; email?: string | null; display?: string | null } | null;
  /** `HostModel.progress`: what a running setup or Sign In waits for. */
  progress?: string | null;
}

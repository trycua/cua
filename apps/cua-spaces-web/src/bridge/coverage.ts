// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * How each host answers every bridge operation (`OPERATIONS`).
 *
 * - `webkit`: the native hosts' methods the adapter calls
 *   (`WebUIBridge.methods`). The SwiftUI app and the Electron shell answer
 *   the same methods (`webkit-protocol.ts`), so this is both hosts'. An
 *   operation that calls none, or that fails on purpose for some arguments,
 *   says why in `byDesign`.
 * - `electron`: only where the Electron shell calls other methods than the
 *   SwiftUI app (its own are `ELECTRON_HOST_METHODS`).
 * - `tauri`: the `invoke` commands the adapter calls.
 * - `unsupported`: hosts with no API for the operation yet, and why. The
 *   adapter rejects there with `unsupported` and asks the host nothing.
 *
 * `__tests__/coverage.test.ts` checks this table against the adapters and
 * against the Swift method list, and fails when webkit misses an operation
 * without a `byDesign` note, when the Swift host routes a method no
 * operation calls and `WEBKIT_HOST_ONLY` doesn't say why, and when an
 * `unsupported` or `byDesign` note has gone stale (the host routes it now).
 * The Swift side checks that every listed method is routed
 * (`BridgeContractTests.swift`).
 */

import { NEW_SPACE_COVERAGE } from "./ops/new-space";
import { SHARE_COVERAGE } from "./ops/share";
import { AGENT_DETAIL_COVERAGE } from "./ops/agent-detail";
import { AGENT_KEYS_COVERAGE } from "./ops/agent-keys";
import { KEYVAULT_MANAGE_COVERAGE } from "./ops/keyvault-manage";
import { KEYVAULT_SETUP_COVERAGE } from "./ops/keyvault-setup";
import { TELEPORT_COVERAGE } from "./ops/teleport";
import { VOLUME_COVERAGE } from "./ops/volume";
import type { OpName } from "./protocol";
import { NOTIFICATIONS_TAURI_COMMANDS as NT } from "./ops/notifications";
import { SETTINGS_TAURI_COMMANDS as ST } from "./ops/settings";
import type { HostMethod, WebkitMethod } from "./webkit-protocol";
import { HOST_SETUP_COVERAGE } from "./ops/host-setup";
import { SPACE_DETAIL_COVERAGE } from "./ops/space-detail";
import { STREAM_COVERAGE } from "./ops/stream";
import { RELAY_USAGE_COVERAGE } from "./ops/relay-usage";
import { NUDGES_COVERAGE } from "./ops/nudges";
import { RESOURCES_COVERAGE } from "./ops/resources";
import { MACHINES_COVERAGE } from "./ops/machines";
import { CHECKLIST_COVERAGE } from "./ops/checklist";
import { LINKS_COVERAGE } from "./ops/links";
import { CLI_COVERAGE } from "./ops/cli";
import { THREAD_COVERAGE } from "./ops/thread";
import { PRESENCE_COVERAGE } from "./ops/presence";
import { CLOUD_APPROVE_COVERAGE } from "./ops/cloud-approve";
import { RUNTIME_FIX_COVERAGE } from "./ops/runtime-fix";
import { TELEMETRY_COVERAGE } from "./ops/telemetry";
import { STARTUP_COVERAGE } from "./ops/startup";

export interface OpCoverage {
  webkit: { methods: readonly WebkitMethod[]; byDesign?: string };
  electron?: { methods: readonly HostMethod[] };
  tauri: readonly string[];
  unsupported?: { webkit?: string; tauri?: string };
}

export const HOST_COVERAGE = {
  "spaces.list": { webkit: { methods: ["spaces.list"] }, tauri: ["list_spaces"] },
  "spaces.create": { webkit: { methods: ["spaces.create"] }, tauri: ["create_space"] },
  "spaces.cancelCreate": { webkit: { methods: ["spaces.cancelCreate"] }, tauri: ["cancel_create"] },
  "spaces.open": {
    webkit: { methods: ["spaces.open"] },
    tauri: ["open_space_window"],
  },
  "spaces.setPower": { webkit: { methods: ["spaces.setPower"] }, tauri: ["set_space_power"] },
  "spaces.delete": { webkit: { methods: ["spaces.delete"] }, tauri: ["delete_space"] },
  "spaces.deleteMany": {
    webkit: { methods: ["spaces.deleteMany"] },
    tauri: [],
    unsupported: { tauri: "the Tauri shell is not the shipped app (SwiftUI and Electron are); it deletes one Space at a time" },
  },
  "spaces.leave": {
    webkit: { methods: ["spaces.leave"] },
    tauri: [],
    unsupported: { tauri: "the Tauri shell is not the shipped app (SwiftUI and Electron are); it has no leave" },
  },
  "spaces.rename": {
    webkit: { methods: ["spaces.rename"] },
    tauri: [],
    unsupported: { tauri: "the Tauri shell is not the shipped app (SwiftUI and Electron are); it has no rename" },
  },
  "machines.list": {
    webkit: { methods: ["spaces.list", "machines.list"] },
    tauri: ["host_status", "list_hosts", "get_environment"],
  },
  "host.status": { webkit: { methods: ["host.status"] }, tauri: ["host_status"] },
  "settings.get": { webkit: { methods: ["settings.get"] }, tauri: ["telemetry_status", "get_default_location", "login_item_status"] },
  "settings.set": {
    webkit: {
      methods: ["settings.choose", "settings.get", "settings.update", "window.setBackgroundColor"],
      byDesign: "theme and hotkey are page-side; New Spaces start on This Mac, or in a linked cloud when This Mac is full",
    },
    tauri: ["ui_storage_set", "telemetry_set_enabled", "set_default_location", "login_item_set", "telemetry_status", "get_default_location", "login_item_status"],
  },
  "settings.choose": {
    webkit: { methods: ["settings.choose", "settings.get"] },
    tauri: [],
    unsupported: { tauri: "the Tauri shell lays out none of these rows" },
  },
  "keyvault.overview": { webkit: { methods: ["keyvault.get"] }, tauri: ["keyvault_overview"] },
  "keyvault.unlock": {
    webkit: { methods: ["keyvault.unlockVault"], byDesign: "the host asks for the passphrase itself: native_only" },
    tauri: ["keyvault_unlock", "keyvault_unlock_passphrase"],
  },
  "keyvault.setUnattended": {
    webkit: { methods: ["keyvault.unlock", "keyvault.lock"] },
    tauri: ["keyvault_set_unattended"],
  },
  "keyvault.setDisabled": { webkit: { methods: ["keyvault.setDisabled"] }, tauri: ["keyvault_set_disabled"] },
  "keyvault.approve": { webkit: { methods: ["keyvault.approve"] }, tauri: ["keyvault_approve"] },
  "keyvault.deny": { webkit: { methods: ["keyvault.deny"] }, tauri: ["keyvault_deny"] },
  "keyvault.revokeGrant": { webkit: { methods: ["keyvault.revokeGrant"] }, tauri: ["keyvault_revoke_grant"] },
  "session.get": {
    webkit: { methods: ["session.get", "onboarding.get"] },
    tauri: ["account_status", "onboarding_state", "daemon_status"],
  },
  "session.signIn": { webkit: { methods: ["session.signIn"] }, tauri: ["begin_sign_in"] },
  "session.signOut": { webkit: { methods: ["session.signOut"] }, tauri: ["sign_out"] },
  "session.completeOnboarding": {
    webkit: { methods: ["onboarding.complete"] },
    tauri: ["complete_onboarding"],
  },
  "session.openExternal": {
    webkit: { methods: [], byDesign: "the page opens the link itself (window.open, which WebKit hands to the browser)" },
    tauri: ["open_external"],
  },

  // The Agents page.
  "agents.list": { webkit: { methods: ["agents.list"] }, tauri: ["agents_tool"] },
  "agents.runs": { webkit: { methods: ["agents.runs"] }, tauri: ["list_space_agents"] },
  "agents.events": {
    webkit: { methods: ["agents.events"] },
    tauri: [],
    unsupported: { tauri: "no Tauri command reads agent_events, and agents_tool doesn't allow it" },
  },
  "agents.start": {
    webkit: { methods: ["agents.start"] },
    tauri: [],
    unsupported: { tauri: "no Tauri command starts a run, and agents_tool doesn't allow agent_start" },
  },
  "agents.message": {
    webkit: { methods: ["agents.message"] },
    tauri: [],
    unsupported: { tauri: "no Tauri command sends to a run, and agents_tool doesn't allow agent_message" },
  },
  "agents.pause": { webkit: { methods: ["agents.pause"] }, tauri: ["agents_tool"] },
  "agents.resume": { webkit: { methods: ["agents.resume"] }, tauri: ["agents_tool"] },
  "agents.setup": { webkit: { methods: ["agents.setup"] }, tauri: ["agent_setup_detect"] },
  "agents.configure": { webkit: { methods: ["agents.configure"] }, tauri: ["agent_setup_configure"] },
  "agents.signIn": { webkit: { methods: ["agents.signIn"] }, tauri: [], unsupported: { tauri: "no command starts an agent's own sign-in" } },
  "agents.status": { webkit: { methods: ["agents.status"] }, tauri: [], unsupported: { tauri: "the Tauri shell has no connection status command (cua connect status --json)" } },
  "agents.connect": { webkit: { methods: ["agents.connect"] }, tauri: [], unsupported: { tauri: "the Tauri shell can't copy an agent's sign-in to Keyvault" } },
  "agents.disconnect": { webkit: { methods: ["agents.disconnect"] }, tauri: [], unsupported: { tauri: "the Tauri shell can't delete a copied sign-in" } },
  "agents.signInStart": { webkit: { methods: ["agents.signInStart"] }, tauri: [], unsupported: { tauri: "the Tauri shell can't start a device sign-in" } },
  "agents.signInWait": { webkit: { methods: ["agents.signInWait"] }, tauri: [], unsupported: { tauri: "the Tauri shell can't start a device sign-in" } },
  ...AGENT_DETAIL_COVERAGE,
  ...NEW_SPACE_COVERAGE,
  ...TELEPORT_COVERAGE,
  ...SHARE_COVERAGE,
  ...AGENT_KEYS_COVERAGE,
  ...KEYVAULT_SETUP_COVERAGE,
  ...KEYVAULT_MANAGE_COVERAGE,
  ...VOLUME_COVERAGE,

  // Settings: About, Experiments, launch at login, Devices, Storage (ops/settings.ts; README, "Settings").
  "about.get": { webkit: { methods: ["about.get"] }, tauri: ST["about.get"] },
  "about.set": { webkit: { methods: ["about.set"] }, tauri: ST["about.set"], unsupported: { tauri: "the Tauri updater has one feed and no controls" } },
  "about.checkNow": { webkit: { methods: ["about.checkNow"] }, tauri: ST["about.checkNow"], unsupported: { tauri: "the Tauri updater has one feed and no controls" } },
  "update.get": { webkit: { methods: ["update.get"] }, tauri: ST["update.get"], unsupported: { tauri: "the Tauri updater has no in-app indicator" } },
  "update.install": { webkit: { methods: ["update.install"] }, tauri: ST["update.install"], unsupported: { tauri: "the Tauri updater has no in-app indicator" } },
  "experiments.get": { webkit: { methods: ["app.info"] }, tauri: ST["experiments.get"] },
  "experiments.set": { webkit: { methods: ["app.info", "settings.choose"] }, tauri: ST["experiments.set"] },
  "loginItem.get": { webkit: { methods: ["loginItem.get"] }, tauri: ST["loginItem.get"] },
  "loginItem.set": { webkit: { methods: ["loginItem.set"] }, tauri: ST["loginItem.set"] },
  "loginItem.openSettings": { webkit: { methods: ["loginItem.openSettings"] }, tauri: ST["loginItem.openSettings"] },
  "runtimes.check": { webkit: { methods: ["runtimes.check"] }, tauri: ST["runtimes.check"], unsupported: { tauri: "the Tauri app has no per-kind runtime settings" } },
  "runtimes.browse": { webkit: { methods: ["runtimes.browse"] }, tauri: ST["runtimes.browse"], unsupported: { tauri: "the Tauri app has no per-kind runtime settings" } },
  "repoCache.get": { webkit: { methods: ["repoCache.get"] }, tauri: ST["repoCache.get"], unsupported: { tauri: "the Tauri app has no repo cache" } },
  "repoCache.set": { webkit: { methods: ["repoCache.set"] }, tauri: ST["repoCache.set"], unsupported: { tauri: "the Tauri app has no repo cache" } },
  "repoCache.clear": { webkit: { methods: ["repoCache.clear"] }, tauri: ST["repoCache.clear"], unsupported: { tauri: "the Tauri app has no repo cache" } },
  "devices.get": { webkit: { methods: ["devices.get"] }, tauri: ST["devices.get"] },
  "devices.enroll": { webkit: { methods: ["devices.enroll"] }, tauri: ST["devices.enroll"] },
  "devices.checkEnrolled": { webkit: { methods: ["devices.checkEnrolled"] }, tauri: ST["devices.checkEnrolled"] },
  "devices.approve": { webkit: { methods: ["devices.approve"] }, tauri: ST["devices.approve"] },
  "devices.rename": { webkit: { methods: ["devices.rename"] }, tauri: ST["devices.rename"] },
  "devices.revoke": { webkit: { methods: ["devices.revoke"] }, tauri: ST["devices.revoke"] },
  "devices.confirmMachine": { webkit: { methods: ["devices.confirmMachine"] }, tauri: ST["devices.confirmMachine"] },
  "devices.createPairingCode": { webkit: { methods: ["devices.createPairingCode"] }, tauri: ST["devices.createPairingCode"] },
  "devices.pairingCodes": { webkit: { methods: ["devices.pairingCodes"] }, tauri: ST["devices.pairingCodes"] },
  "devices.revokePairingCode": { webkit: { methods: ["devices.revokePairingCode"] }, tauri: ST["devices.revokePairingCode"] },
  "teams.list": { webkit: { methods: ["teams.list"] }, tauri: ST["teams.list"] },
  "storage.get": { webkit: { methods: ["storage.get"] }, tauri: ST["storage.get"] },
  "storage.run": { webkit: { methods: ["storage.run"] }, tauri: ST["storage.run"] },
  // Notifications (ops/notifications.ts).
  "notifications.list": { webkit: { methods: ["notifications.list"] }, tauri: NT["notifications.list"] },
  "notifications.markAllRead": { webkit: { methods: ["notifications.markAllRead"] }, tauri: NT["notifications.markAllRead"] },
  "notifications.markRead": { webkit: { methods: ["notifications.markRead"] }, tauri: NT["notifications.markRead"] },
  ...TELEMETRY_COVERAGE,
  ...SPACE_DETAIL_COVERAGE,
  ...HOST_SETUP_COVERAGE,
  ...STARTUP_COVERAGE,
  ...STREAM_COVERAGE,
  ...RELAY_USAGE_COVERAGE,
  ...NUDGES_COVERAGE,
  ...RESOURCES_COVERAGE,
  ...MACHINES_COVERAGE,
  ...CHECKLIST_COVERAGE,
  ...LINKS_COVERAGE,
  ...CLI_COVERAGE,
  ...THREAD_COVERAGE,
  ...PRESENCE_COVERAGE,
  ...CLOUD_APPROVE_COVERAGE,
  ...RUNTIME_FIX_COVERAGE,
} as const satisfies Record<OpName, OpCoverage>;

/**
 * Methods the SwiftUI host routes that no operation calls, and who calls
 * them instead.
 */
export const WEBKIT_HOST_ONLY: Partial<Record<WebkitMethod, string>> = {
  "window.setDragRegions": "the webkit adapter sends the page's drag regions itself whenever the layout changes (drag-regions.ts)",
  "window.setDropTargets": "the webkit adapter sends the Space tiles' boxes itself whenever the layout changes, for a real window dragged onto one (window-drop.ts)",
};

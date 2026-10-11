// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The operations every host answers, and the events it may push.
 *
 * One table for all hosts: the Tauri adapter maps each operation onto the
 * existing `invoke` commands, the native hosts (the SwiftUI WKWebView and
 * the Electron shell) answer them as messages (`webkit-protocol.ts`), and
 * the demo adapter answers them in
 * memory. Every operation is an existing Tauri command (or a small group of
 * them) under a host-neutral name; nothing here is a new mechanism.
 */

import type {
  DaemonStatus,
  DefaultLocation,
  FleetStatus,
  HostStatus,
  LoginItemStatus,
  MachineRow,
  OnboardingMode,
  OnboardingState,
  SignInStart,
  SpaceCreateConfig,
  SpacePowerReport,
  CancelOutcome,
  CreateProgress,
  TelemetryView,
} from "./contracts/host";
import type { AgentEventsPage, AgentSetupRow, PersistentAgent, SpaceAgentRun } from "./contracts/agents";
import type { KeyvaultOverview, KvGrant, KvItem } from "./contracts/keyvault";
import type { SpaceOs, SpaceRow } from "./contracts/spaces";
import { NEW_SPACE_OPERATIONS, type NewSpaceOperations } from "./ops/new-space";
import { SHARE_OPERATIONS, type ShareOperations } from "./ops/share";
import { AGENT_KEYS_OPERATIONS } from "./ops/agent-keys";
import { KEYVAULT_MANAGE_OPERATIONS } from "./ops/keyvault-manage";
import { KEYVAULT_SETUP_OPERATIONS } from "./ops/keyvault-setup";
import { TELEPORT_EVENTS, TELEPORT_OPERATIONS, type TeleportHostEvent, type TeleportOperations } from "./ops/teleport";
import { VOLUME_OPERATIONS } from "./ops/volume";
import { NOTIFICATIONS_OPERATIONS, type NotificationsOperations } from "./ops/notifications";
import { SETTINGS_OPERATIONS, type SettingsOperations } from "./ops/settings";
import type { HostSetupOps } from "./ops/host-setup";
import type { SpaceDetailOps } from "./ops/space-detail";
import { STREAM_OPERATIONS, type StreamOps } from "./ops/stream";
import type { TelemetryOps } from "./ops/telemetry";
import { STARTUP_EVENTS, STARTUP_OPERATIONS, type StartupHostEvent, type StartupOps } from "./ops/startup";

/* ---- Settings the web UI reads and writes -------------------------------- */

export type AppearanceTheme = "system" | "light" | "dark";

/** The settings `useSettings()` exposes; each key is one `updateSetting`. */
export interface SettingsValues {
  /** Light, dark or follow the system. UI storage key `cua.settings.appearance`. */
  theme: AppearanceTheme;
  /** The menu bar item is the entry point instead of the notch. */
  menuBar: boolean;
  /** The global hotkey, as glyphs (`⌘⇧Space`). */
  hotkey: string;
  /** Anonymous usage telemetry (`$CUA_HOME/config.toml`, same as `cua telemetry`). */
  telemetry: boolean;
  /** Where New Space starts: `local`, `cloud`, `host:<id>`, or a cloud word. */
  defaultLocation: string;
  /** The app opens at login, as the system reports it (`login_item::LoginItemStatus`
   * is `enabled` or `requiresApproval`). Null: this host can't say. */
  launchAtLogin: boolean | null;
  /** Which releases the updater offers (`about::UpdateChannel`). Null: this
   * host has no channel choice. */
  updateChannel: UpdateChannel | null;
  /** "Connect to the desktop automatically" (`AppSettings.auto_connect`,
   * on by default): a Space's detail opens its live desktop at once, else
   * shows Connect. Absent where the host has no such setting (on). */
  autoConnect?: boolean;
}

export type UpdateChannel = "stable" | "beta";

/** A login item status as the switch shows it (`LoginItemStatus::is_on`);
 * null when unknown or when the system can't find the app. */
export function loginItemOn(status: LoginItemStatus | null | undefined): boolean | null {
  if (!status || status === "notFound") return null;
  return status === "enabled" || status === "requiresApproval";
}

export type SettingKey = keyof SettingsValues;

export interface SettingsSnapshot {
  values: SettingsValues;
  /** Where `defaultLocation` comes from (env-locked or not), when known. */
  defaultLocation: DefaultLocation | null;
  /** The telemetry switch with its source, when known. */
  telemetry: TelemetryView | null;
  /** Settings only some hosts have (the SwiftUI app), as its Settings page
   * laid them out; each absent or null one is not this host's, and the page
   * leaves its row out. Changed with `settings.choose`. */
  hostSettings?: HostSettings;
}

/** `settings::SettingsInput`'s host-only fields: the core lays out their rows. */
export interface HostSettings {
  /** Which Lume macOS Spaces run on (`auto`, `builtin`, `system`). */
  lumeSource?: string | null;
  /** Which engine local Linux Spaces run on (`auto`, `builtin`, `system`). */
  linuxSource?: string | null;
  /** "Connect to the desktop automatically". */
  autoConnect?: boolean | null;
  /** The Keyvault wipes access from Spaces automatically. */
  keyvaultAutoWipe?: boolean | null;
}

export const DEFAULT_SETTINGS: SettingsValues = {
  theme: "system",
  menuBar: false,
  hotkey: "⌘⇧Space",
  telemetry: true,
  defaultLocation: "local",
  launchAtLogin: null,
  updateChannel: null,
};

/** UI-only settings live in the shell's UI storage under these keys. */
export const UI_STORAGE_KEYS = {
  theme: "cua.settings.appearance",
  menuBar: "cua.settings.menuBar",
  hotkey: "cua.settings.hotkey",
} as const;

/* ---- Session ------------------------------------------------------------- */

export interface SessionSnapshot {
  fleet: FleetStatus;
  onboarding: OnboardingState;
  daemon: DaemonStatus | null;
  /** A native host's sign-in waiting for the browser (it runs the flow
   * itself): the code to confirm and the page it finishes on. */
  hostSignIn?: HostSignIn | null;
}

/** A sign-in the host waits on (`SessionSnapshot.hostSignIn`). */
export interface HostSignIn {
  userCode: string | null;
  url: string | null;
}

/* ---- Operations ---------------------------------------------------------- */

/**
 * `op name -> { args, result }`. The Tauri command each one maps to is in
 * `adapters/tauri.ts`; the README has the full table.
 */
export interface HostOperations
  extends NewSpaceOperations,
    TeleportOperations,
    ShareOperations,
    SettingsOperations,
    NotificationsOperations,
    TelemetryOps,
    SpaceDetailOps,
    HostSetupOps,
    StartupOps,
    StreamOps {
  "spaces.list": { args: Record<string, never>; result: SpaceRow[] };
  /** `os`: the plan's OS, for a host that draws its own pending row (the SwiftUI app's notch). */
  "spaces.create": { args: { config: SpaceCreateConfig; pendingId: string; os?: SpaceOs }; result: SpaceRow };
  "spaces.cancelCreate": { args: { pendingId: string }; result: CancelOutcome };
  "spaces.setPower": { args: { spaceId: string; on: boolean }; result: SpacePowerReport };
  /** Deletes the Space, or with `removeOnly` only forgets it here (a Space
   * in your cloud keeps running there: the confirm's "Remove from List"). */
  "spaces.delete": { args: { spaceId: string; removeOnly?: boolean }; result: string };
  /** Opens (or focuses) the Space's own desktop window, natively. `name`
   * and `os` title the window where the host wants them (Tauri). */
  "spaces.open": { args: { spaceId: string; name?: string; os?: SpaceOs }; result: null };

  "machines.list": { args: Record<string, never>; result: MachineRow[] };
  "host.status": { args: Record<string, never>; result: HostStatus };

  "settings.get": { args: Record<string, never>; result: SettingsSnapshot };
  "settings.set": { args: { key: SettingKey; value: SettingsValues[SettingKey] }; result: SettingsSnapshot };
  /** Picks an option on one of the host's own rows (`SettingsSnapshot.hostSettings`:
   * `macos-runtime`, `linux-runtime`, `auto-connect`, `keyvault-auto-wipe`),
   * as the core's Settings page names them. The SwiftUI host also takes
   * `welcome` (General's "Show again": its welcome window). */
  "settings.choose": { args: { row: string; option: string }; result: SettingsSnapshot };

  "keyvault.overview": { args: Record<string, never>; result: KeyvaultOverview };
  /** Unlocks the vault: the OS protector (Touch ID, native) or a passphrase,
   * which goes only to the broker. */
  "keyvault.unlock": { args: { passphrase?: string | null }; result: null };
  "keyvault.setUnattended": { args: { itemIds: string[]; unattended: boolean }; result: KvItem[] };
  "keyvault.setDisabled": { args: { disabled: boolean }; result: null };
  "keyvault.approve": { args: { requestId: string; items: string[] | null }; result: KvGrant };
  "keyvault.deny": { args: { requestId: string }; result: null };
  "keyvault.revokeGrant": { args: { id: string }; result: number };

  "session.get": { args: Record<string, never>; result: SessionSnapshot };
  "session.signIn": { args: Record<string, never>; result: SignInStart };
  "session.signOut": { args: Record<string, never>; result: null };
  /** `launchAtLogin`: Done's checkbox, where the host draws it (Electron). */
  "session.completeOnboarding": { args: { mode: OnboardingMode; launchAtLogin?: boolean }; result: null };
  "session.openExternal": { args: { url: string }; result: null };

  /** Persistent agents (`persistent_agent_list`). */
  "agents.list": { args: Record<string, never>; result: PersistentAgent[] };
  /** Agent runs in one Space, newest first (`list_space_agents`). */
  "agents.runs": { args: { spaceId: string }; result: SpaceAgentRun[] };
  /** A run's events after `cursor` (`agent_events`; at most `max`, default 100). */
  "agents.events": { args: { spaceId: string; runId: string; cursor: number; max?: number }; result: AgentEventsPage };
  /** Stops the run, saves the home, holds routines, suspends a local Space (`agent_pause`). */
  "agents.pause": { args: { name: string }; result: null };
  "agents.resume": { args: { name: string }; result: null };
  /** The coding agents on this machine and what cua set up for each (`agent_setup_detect`). */
  "agents.setup": { args: Record<string, never>; result: AgentSetupRow[] };
  /** Adds the cua skills and MCP server to `agents`, or every installed one (`agent_setup_configure`). */
  "agents.configure": { args: { agents: string[] | null }; result: AgentSetupRow[] };
}

export type OpName = keyof HostOperations;
export type OpArgs<K extends OpName> = HostOperations[K]["args"];
export type OpResult<K extends OpName> = HostOperations[K]["result"];

/** Every operation, for hosts to check coverage against. */
export const OPERATIONS = [
  "spaces.list",
  "spaces.create",
  "spaces.cancelCreate",
  "spaces.setPower",
  "spaces.delete",
  "spaces.open",
  "machines.list",
  "host.status",
  "settings.get",
  "settings.set",
  "settings.choose",
  "keyvault.overview",
  "keyvault.unlock",
  "keyvault.setUnattended",
  "keyvault.setDisabled",
  "keyvault.approve",
  "keyvault.deny",
  "keyvault.revokeGrant",
  "session.get",
  "session.signIn",
  "session.signOut",
  "session.completeOnboarding",
  "session.openExternal",
  "agents.list",
  "agents.runs",
  "agents.events",
  "agents.pause",
  "agents.resume",
  "agents.setup",
  "agents.configure",
  ...NEW_SPACE_OPERATIONS,
  ...TELEPORT_OPERATIONS,
  ...SHARE_OPERATIONS,
  ...AGENT_KEYS_OPERATIONS,
  ...KEYVAULT_SETUP_OPERATIONS,
  ...KEYVAULT_MANAGE_OPERATIONS,
  ...VOLUME_OPERATIONS,
  ...STREAM_OPERATIONS,
  ...SETTINGS_OPERATIONS,
  ...NOTIFICATIONS_OPERATIONS,
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
  ...STARTUP_OPERATIONS,
] as const satisfies readonly OpName[];

/* ---- Events a host pushes ------------------------------------------------ */

export type HostEvent =
  /** The registry changed: refetch `spaces.list` (Tauri `spaces:changed`). */
  | { type: "spaces.changed" }
  /** A create moved on (Tauri `spaces:create-progress`). */
  | { type: "spaces.createProgress"; progress: CreateProgress }
  | { type: "machines.changed" }
  | { type: "settings.changed" }
  | { type: "keyvault.changed" }
  /** Tauri `auth:signed-in`. */
  | { type: "session.signedIn"; identity?: string }
  /** Tauri `auth:sign-in-failed`. */
  | { type: "session.signInFailed"; reason: string }
  /** Tauri `auth:signed-out`. */
  | { type: "session.signedOut" }
  /** The account or a sign-in moved on (the SwiftUI host's `session.changed`): refetch. */
  | { type: "session.changed" }
  /** A persistent agent or a run changed (paused, resumed, a turn ended): refetch. */
  | { type: "agents.changed" }
  | TeleportHostEvent
  /** The native app moved on while starting (the startup screen; payload: the new state). */
  | StartupHostEvent;

export type HostEventType = HostEvent["type"];

export const HOST_EVENTS = [
  "spaces.changed",
  "spaces.createProgress",
  "machines.changed",
  "settings.changed",
  "keyvault.changed",
  "session.signedIn",
  "session.signInFailed",
  "session.signedOut",
  "session.changed",
  "agents.changed",
  ...TELEPORT_EVENTS,
  ...STARTUP_EVENTS,
] as const satisfies readonly HostEventType[];

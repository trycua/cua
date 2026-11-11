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
  AccountStatus,
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
  RuntimesInput,
} from "./contracts/host";
import type {
  AgentEventsPage,
  AgentMessageSent,
  AgentSetupRow,
  AgentStarted,
  AgentStartRequest,
  ConnectionReport,
  ConnectResult,
  PersistentAgent,
  SpaceAgentRun,
} from "./contracts/agents";
import type { KeyvaultOverview, KvGrant, KvItem } from "./contracts/keyvault";
import type { SpaceOs, SpaceRow } from "./contracts/spaces";
import type { BulkResult } from "./contracts/spaces-page";
import { NEW_SPACE_OPERATIONS, type NewSpaceOperations } from "./ops/new-space";
import { SHARE_OPERATIONS, type ShareOperations } from "./ops/share";
import { AGENT_KEYS_OPERATIONS } from "./ops/agent-keys";
import { GITHUB_OPERATIONS } from "./ops/github";
import { AGENT_DETAIL_OPERATIONS } from "./ops/agent-detail";
import { KEYVAULT_MANAGE_OPERATIONS } from "./ops/keyvault-manage";
import { KEYVAULT_SETUP_OPERATIONS } from "./ops/keyvault-setup";
import { TELEPORT_EVENTS, TELEPORT_OPERATIONS, type TeleportHostEvent, type TeleportOperations } from "./ops/teleport";
import { VOLUME_OPERATIONS } from "./ops/volume";
import { NOTIFICATIONS_OPERATIONS, type NotificationsOperations } from "./ops/notifications";
import { SETTINGS_OPERATIONS, type SettingsOperations } from "./ops/settings";
import type { HostSetupOps } from "./ops/host-setup";
import type { SpaceDetailOps } from "./ops/space-detail";
import { STREAM_OPERATIONS, type StreamOps } from "./ops/stream";
import { RELAY_USAGE_OPERATIONS, type RelayUsageOps } from "./ops/relay-usage";
import { NUDGES_OPERATIONS, type NudgesOps } from "./ops/nudges";
import { RESOURCES_OPERATIONS, type ResourcesOps } from "./ops/resources";
import { MACHINES_OPERATIONS, type MachinesOps } from "./ops/machines";
import { SPACES_SETTINGS_OPERATIONS, type SpacesSettingsOps } from "./ops/spaces-settings";
import { CHECKLIST_OPERATIONS, type ChecklistOps } from "./ops/checklist";
import { LINKS_OPERATIONS, type LinksOps } from "./ops/links";
import { CLI_OPERATIONS, type CliOps } from "./ops/cli";
import { CI_OPERATIONS, type CiOps } from "./ops/ci";
import { THREAD_OPERATIONS, type ThreadOps } from "./ops/thread";
import { PRESENCE_OPERATIONS, type PresenceOps } from "./ops/presence";
import { CLOUD_APPROVE_OPERATIONS, type CloudApproveOps } from "./ops/cloud-approve";
import { AGENT_THREADS_OPERATIONS, type AgentThreadsOps } from "./ops/agent-threads";
import { AGENT_SIGN_IN_OPERATIONS, type AgentSignInOps } from "./ops/agent-sign-in";
import { RUNTIME_FIX_OPERATIONS, type RuntimeFixOps } from "./ops/runtime-fix";
import { DRIVER_OPERATIONS, type DriverOps } from "./ops/driver";
import { TELEMETRY_OPERATIONS, type TelemetryOps } from "./ops/telemetry";
import type { TipsState } from "./contracts/spaces-page";
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
  /** Usage telemetry (`$CUA_HOME/config.toml`, same as `cua telemetry`). */
  telemetry: boolean;
  /** Where New Space starts: `local`, `host:<id>`, or a cloud word. */
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
  /** Tips (`AppSettings.tips`): off, and the ones done. Absent where the
   * host keeps no core settings (the page keeps them for this session). */
  tips?: TipsState;
  /** The Spaces and machine desktops pinned to the notch (`AppSettings.notch_pinned`),
   * in pin order. Absent where the host keeps no core settings (the page keeps them
   * for this session). */
  notchPinned?: string[];
  /** Which coding apps Home's agent menu shows (`AppSettings.agentMenu*`, the core's
   * `agent_menu`). Absent where the host keeps no core settings (the page keeps it
   * for this session). */
  agentMenu?: AgentMenuPrefs;
}

/** The apps whose default the person changed (`agent_menu::AgentMenuPrefs`). */
export interface AgentMenuPrefs {
  shown: string[];
  hidden: string[];
  seen: string[];
}

export type UpdateChannel = "stable" | "nightly";

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
  /** The global shortcut that brings the app to the front, as the host
   * registered it with the system. Absent or null: this host has none. */
  globalShortcut?: GlobalShortcut | null;
  /** The host's account app (`https://app.cua.ai`, or staging's), which the
   * page's core cannot resolve itself. Absent or null: the production one. */
  accountUrl?: string | null;
  /** The CLI this app puts on PATH, for the pages' "run this in Terminal"
   * hints: its name (`CuaChannel::cli_name`, from the host's CLI install
   * plan) and whether it is on this computer. Absent or null: the host
   * can't say, and the hints stay hidden. */
  cli?: CliInfo | null;
  /** The channel and environment this app runs as, as the host resolved them
   * (`app_target`, from cua-home): the title bar's "Local · Staging" tag
   * (the core's `environment.tag`). Absent or null: the host can't say, and
   * no tag shows. */
  target?: AppTarget | null;
}

/** `SettingsSnapshot.target` (`environment_tag::Target` in the app core). */
export interface AppTarget {
  /** `stable` or `local`. */
  channel: string;
  /** `prod` or `staging`. */
  env: string;
  /** The sign-in server's host (`auth.staging.cua.ai`); empty: the environment's. */
  authHost: string;
}

/** `SettingsSnapshot.cli`. */
export interface CliInfo {
  /** The command to type (`cua` for the released app). */
  name: string;
  /** It is installed on this computer. */
  installed: boolean;
}

/** A system-wide shortcut the host registered (macOS: ⌘⇧Space). */
export interface GlobalShortcut {
  /** As glyphs (`⌘⇧Space`). */
  label: string;
  /** False when the system refused it (another app holds the same keys). */
  registered: boolean;
}

/** `settings::SettingsInput`'s host-only fields: the core lays out their rows. */
export interface HostSettings {
  /** Settings, Advanced's runtimes, as the host read them (`runtimes::RuntimesInput`). */
  runtimes?: RuntimesInput | null;
  /** "Connect to the desktop automatically". */
  autoConnect?: boolean | null;
  /** New Spaces overflow into your linked cloud when this machine is full (`placement.overflow`). */
  overflow?: boolean | null;
  /** The Mac app's background service: the close button keeps Cua Spaces running (none: no rows). */
  closeWindowKeepsRunning?: boolean | null;
  /** The notice that Cua Spaces keeps running is on. */
  closeNotice?: boolean | null;
  /** The Keyvault wipes access from Spaces automatically. */
  keyvaultAutoWipe?: boolean | null;
  /** The Keyvault explains unattended access before allowing it (off after "Never ask again"). */
  keyvaultUnlockPrompt?: boolean | null;
  /** The Keyvault loads site icons the browser had none for. */
  keyvaultSiteIcons?: boolean | null;
  /** The Keyvault's protection facts, as the host's Settings lists them. */
  keyvaultProtection?: { label: string; value: string }[] | null;
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
  account: AccountStatus;
  onboarding: OnboardingState;
  daemon: DaemonStatus | null;
  /** A native host's sign-in waiting for the browser (it runs the flow
   * itself): the code to confirm and the page it finishes on. */
  hostSignIn?: HostSignIn | null;
  /** A native host's sign-in that failed (its reason, the core's line and
   * the error's words for Details): the page shows it, never a wait that
   * goes on. Absent from an older host. */
  hostSignInFailure?: HostSignInFailure | null;
}

/** A sign-in the host waits on (`SessionSnapshot.hostSignIn`). */
export interface HostSignIn {
  userCode: string | null;
  url: string | null;
  /** A browser opened on the host (false: open the page by hand). */
  opened?: boolean;
}

/** A host sign-in that failed (`SessionSnapshot.hostSignInFailure`). */
export interface HostSignInFailure {
  reason: string;
  message: string;
  detail: string | null;
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
    StreamOps,
    RelayUsageOps,
    NudgesOps,
    ResourcesOps,
    MachinesOps,
    SpacesSettingsOps,
    ChecklistOps,
    LinksOps,
    CliOps,
    CiOps,
    ThreadOps,
    PresenceOps,
    CloudApproveOps,
    AgentThreadsOps,
    AgentSignInOps,
    RuntimeFixOps,
    DriverOps {
  "spaces.list": { args: Record<string, never>; result: SpaceRow[] };
  /** `os`: the plan's OS, for a host that draws its own pending row (the SwiftUI app's notch).
   * `config.children` and `config.labels` (an agent home, CUA-2630) are forwarded to
   * the SDK's create as they are by every host: Electron (`createExtras`), SwiftUI (`CreateExtras`)
   * and Tauri (`SpaceCreateConfig`). */
  "spaces.create": { args: { config: SpaceCreateConfig; pendingId: string; os?: SpaceOs; notifyWhenReady?: boolean }; result: SpaceRow };
  "spaces.cancelCreate": { args: { pendingId: string }; result: CancelOutcome };
  "spaces.setPower": { args: { spaceId: string; on: boolean }; result: SpacePowerReport };
  /** Deletes the Space, or with `removeOnly` only forgets it here (a Space
   * in your cloud keeps running there: the confirm's "Remove from List"). */
  "spaces.delete": { args: { spaceId: string; removeOnly?: boolean }; result: string };
  /** Deletes several Spaces (running ones are turned off first), one result
   * per id in order; one failure never stops the rest. */
  "spaces.deleteMany": { args: { ids: string[] }; result: { results: BulkResult[] } };
  /** Takes you off Spaces shared with you (a team's leaves your list), one
   * result per id in order. */
  "spaces.leave": { args: { ids: string[] }; result: { results: BulkResult[] } };
  /** Gives the Space a new name; its id stays the same. Only its owner can
   * (a Space shared with you is refused in plain words). A Space you shared,
   * or one of your computers provides, is renamed on the relay too. */
  "spaces.rename": { args: { spaceId: string; name: string }; result: null };
  /** A Space's network as `cua network show` says it (the SDK's
   * `Spaces.network`), with its clipboard and Volume settings. */
  "network.get": { args: { spaceId: string }; result: SpaceNetworkWire };
  /** Replaces a Space's network (`cua network set`): `network` is the
   * preset word (`internet`, `none`, `localhost:5432,3000`, `localhost:all`,
   * `lan`, `localhost+lan:5432,3000` (both extras), `custom`, `custom:none`,
   * `unprotected`); `widen` lets a child
   * reach more than its parent (the user's own choice, recorded). */
  "network.set": {
    args: {
      spaceId: string;
      network: string;
      allow: string[];
      deny: string[];
      widen?: boolean;
      tree?: boolean;
      /** Where its internet traffic leaves: `internet`, `this-machine`,
       * `space:<id>` or `socks5://host:port`. Left out: the exit it has. */
      via?: string;
    };
    result: { applied: "now" | "next_start" };
  };
  /** What a Space tried to reach and its network blocked, most recent first. */
  "network.log": { args: { spaceId: string }; result: NetworkEventWire[] };
  /** The one-time notice about the new network default was shown. */
  "network.ackNotice": { args: { spaceId: string }; result: null };
  /** Sets a Space's clipboard (`viewing`, `ask`, `off`) and/or Volume
   * (`own`, `off`) setting. `home` (CUA-2630): what the person approved for an agent home while it
   * runs, written to the cua home's `agent-home/grants.json` where this machine's daemon reads it (`mentions`: Spaces and machines @mentioned in a later request, which count as the grant)
   * (SwiftUI and Electron; the Tauri shell does not answer this operation). */
  "spaces.setAccess": {
    args: { spaceId: string; clipboard?: string; volume?: string; home?: { listAll?: boolean; machines?: string[]; mentions?: string[] } };
    result: null;
  };
  /** Every Space's place in its tree, and whether it may make children
   * (the SDK's `Spaces.list`: `parent`, `depth`, `children`). */
  "spaces.tree": { args: Record<string, never>; result: TreeNodeWire[] };
  /** Lets a Space make child Spaces of its own, or stops it
   * (`cua spaces children <space> on|off`). */
  "children.set": { args: { spaceId: string; enabled: boolean }; result: null };
  /** The budget of a Space's tree and what its children use
   * (`cua spaces budget <space>`). */
  "budget.get": { args: { spaceId: string }; result: TreeBudgetWire };
  /** Sets the budget of a Space's tree; unset limits take the defaults. */
  "budget.set": { args: { spaceId: string; limits: TreeBudgetLimitsWire }; result: TreeBudgetWire };
  /** The commands a Space keeps running (`cua spaces service ls`). */
  "services.list": { args: { spaceId: string }; result: ServiceWire[] };
  /** Starts, stops, restarts or removes one (`cua spaces service ...`). */
  "services.control": { args: { spaceId: string; name: string; action: "start" | "stop" | "restart" | "remove" }; result: null };
  /** Opens (or focuses) the Space's own desktop window, natively. `name`
   * and `os` title the window where the host wants them (Tauri). */
  "spaces.open": { args: { spaceId: string; name?: string; os?: SpaceOs }; result: null };

  "machines.list": { args: Record<string, never>; result: MachineRow[] };
  /** `localNetwork`: the permissions page is asking, so a Mac that runs macOS Spaces also lists its
   * Local Network access ("local-network", granted or not) and checks it from the daemon (the check makes macOS ask
   * the first time). `localNetworkCheck: false` lists the row without checking: the first run checks only once the
   * person pressed Allow, right after the step explains why. */
  "host.status": { args: { localNetwork?: boolean; localNetworkCheck?: boolean }; result: HostStatus };

  "settings.get": { args: Record<string, never>; result: SettingsSnapshot };
  "settings.set": { args: { key: SettingKey; value: SettingsValues[SettingKey] }; result: SettingsSnapshot };
  /** Picks an option on one of the host's own rows (`SettingsSnapshot.hostSettings`:
   * `runtime:<kind>`, `runtime-detected:<kind>`, `runtime-path:<kind>`,
   * `auto-connect`, `keyvault-auto-wipe`),
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
  "session.signIn": { args: { code?: boolean }; result: SignInStart };
  "session.signOut": { args: Record<string, never>; result: null };
  /** `launchAtLogin`: Done's checkbox, where the host draws it (Electron). */
  "session.completeOnboarding": { args: { mode: OnboardingMode; launchAtLogin?: boolean }; result: null };
  "session.openExternal": { args: { url: string }; result: null };
  /** Settings, About, "Reset & Show": the host forgets the first run (its `onboarding.json`); the page clears its own (`onboarding-reset.ts`). */
  "session.resetOnboarding": { args: Record<string, never>; result: null };

  /** Persistent agents (`persistent_agent_list`). */
  "agents.list": { args: Record<string, never>; result: PersistentAgent[] };
  /** Agent runs in one Space, newest first (`list_space_agents`). */
  "agents.runs": { args: { spaceId: string }; result: SpaceAgentRun[] };
  /** A run's events after `cursor` (`agent_events`; at most `max`, default 100). */
  "agents.events": { args: { spaceId: string; runId: string; cursor: number; max?: number }; result: AgentEventsPage };
  /** Starts a coding agent in a Space (`agent_start`). */
  "agents.start": { args: AgentStartRequest; result: AgentStarted };
  /** Sends a reply to a run (`agent_message`; `force` interrupts the turn first). */
  "agents.message": { args: { spaceId: string; runId: string; text: string; force?: boolean }; result: AgentMessageSent };
  /** Stops the run, saves the home, holds routines, suspends a local Space (`agent_pause`). */
  "agents.pause": { args: { name: string }; result: null };
  "agents.resume": { args: { name: string }; result: null };
  /** The coding agents on this machine and what cua set up for each (`agent_setup_detect`). */
  "agents.setup": { args: Record<string, never>; result: AgentSetupRow[] };
  /** Adds the cua skills and MCP server to `agents`, or every installed one (`agent_setup_configure`). */
  "agents.configure": { args: { agents: string[] | null }; result: AgentSetupRow[] };
  /** Starts `agent`'s own sign-in on this computer (`codex login` opens the browser); the rows after. */
  "agents.signIn": { args: { agent: string }; result: AgentSetupRow[] };
  /** Whether each agent on This Mac is really connected (`AgentSetup.connectionStatusJson`, `cua connect status --json`); `refresh` skips the few-second cache. */
  "agents.status": { args: { refresh?: boolean }; result: ConnectionReport };
  /** Connect and Reconnect: Cua's tools and, with `signIn`, the agent's sign-in copied to Keyvault on This Mac (`cua connect setup --sign-in`). */
  "agents.connect": { args: { agents: string[]; signIn: boolean; skills?: boolean; mcp?: boolean }; result: ConnectResult[] };
  /** Disconnect: Cua's entry out of the agent's config (the previous one back) and, with `signIn`, the copy in Keyvault (`cua connect remove --sign-in`). */
  "agents.disconnect": { args: { agents: string[]; signIn: boolean; skills?: boolean; mcp?: boolean }; result: { removed_sign_ins: string[] } };
  /** Starts an agent's device-code sign-in made for Cua (Codex: `codex login --device-auth` in a folder Cua owns): the link and the code to show. */
  "agents.signInStart": { args: { agent: string }; result: { id: string; url: string; code: string } };
  /** Waits for that sign-in to be approved and saves it in Keyvault on This Mac (as Connect's `sign_in`). */
  "agents.signInWait": {
    args: { id: string };
    result: {
      imported: { source: string; env: string; label: string; expires_ms: number | null; added_ms: number } | null;
      refused: string | null;
      line: string | null;
    };
  };
}

export type OpName = keyof HostOperations;
export type OpArgs<K extends OpName> = HostOperations[K]["args"];
export type OpResult<K extends OpName> = HostOperations[K]["result"];

/** A Space's network as the host's SDK says it (`SpaceNetworkInfo`, camelCase),
 * plus its clipboard and Volume settings. */
export interface SpaceNetworkWire {
  preset: string;
  localhostPorts?: number[];
  allLocalhostPorts?: boolean;
  allow?: string[];
  deny?: string[];
  customBase?: string;
  widenedBy?: string | null;
  summary: string;
  widenings?: string[];
  /** `enforced`, `no_network`, `open`, `refused`, or empty. */
  enforcement: string;
  point: string;
  note?: string | null;
  parent?: string | null;
  root?: string | null;
  alwaysBlocked?: string[];
  noticePending: boolean;
  /** Where its internet traffic leaves; null or absent: its own network. */
  via?: string | null;
  /** Its topology as JSON text (`cua network show --json`'s `topology`). */
  topologyJson?: string;
  /** `viewing`, `ask` or `off`. */
  clipboard: string;
  /** `own` or `off`. */
  volume: string;
}

/** A Space's place in its tree (the SDK's `SpaceInfo`, camelCase). */
export interface TreeNodeWire {
  id: string;
  parent?: string | null;
  depth: number;
  /** It may make child Spaces of its own. */
  children: boolean;
  /** It is an agent's home (the Space made for ask-first, labelled `cua.home`): the agent it was made for.
   * Absent or null: it is not one. The host reads it from the Space's labels, so a home is found again on
   * a new device or after the app's storage is cleared. */
  home?: string | null;
}

/** A tree's limits (the SDK's `TreeBudgetLimits`, camelCase); unset: the default. */
export interface TreeBudgetLimitsWire {
  maxSpaces?: number | null;
  maxVcpus?: number | null;
  maxMemoryMb?: number | null;
  usdPerHour?: number | null;
  usdTotal?: number | null;
}

/** A tree's budget and use (the SDK's `TreeBudget`, camelCase). */
export interface TreeBudgetWire {
  root: string;
  set: TreeBudgetLimitsWire;
  maxSpaces: number;
  maxVcpus: number;
  maxMemoryMb: number;
  usdPerHour: number;
  usdTotal?: number | null;
  spaces: number;
  vcpus: number;
  memoryMb: number;
  usdPerHourUsed: number;
  usdSpent: number;
}

/** A kept command (the SDK's `SpaceServiceInfo`, camelCase). */
export interface ServiceWire {
  name: string;
  command: string;
  state: string;
  restarts: number;
  lastEnd: string;
  probeError: string;
}

/** One kind of blocked connection (the SDK's `NetworkEvent`, camelCase). */
export interface NetworkEventWire {
  reason: string;
  dst: string;
  proto?: string | null;
  count: number;
  /** Unix seconds; null when unknown. */
  lastUnix?: number | null;
}

/** Every operation, for hosts to check coverage against. */
export const OPERATIONS = [
  "spaces.list",
  "spaces.create",
  "spaces.cancelCreate",
  "spaces.setPower",
  "spaces.delete",
  "spaces.deleteMany",
  "spaces.leave",
  "spaces.rename",
  "network.get",
  "network.set",
  "network.log",
  "network.ackNotice",
  "spaces.setAccess",
  "spaces.tree",
  "children.set",
  "budget.get",
  "budget.set",
  "services.list",
  "services.control",
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
  "session.resetOnboarding",
  "agents.list",
  "agents.runs",
  "agents.events",
  "agents.start",
  "agents.message",
  "agents.pause",
  "agents.resume",
  "agents.setup",
  "agents.configure",
  "agents.signIn",
  "agents.status",
  "agents.connect",
  "agents.disconnect",
  "agents.signInStart",
  "agents.signInWait",
  ...AGENT_DETAIL_OPERATIONS,
  ...NEW_SPACE_OPERATIONS,
  ...TELEPORT_OPERATIONS,
  ...SHARE_OPERATIONS,
  ...AGENT_KEYS_OPERATIONS,
  ...GITHUB_OPERATIONS,
  ...KEYVAULT_SETUP_OPERATIONS,
  ...KEYVAULT_MANAGE_OPERATIONS,
  ...VOLUME_OPERATIONS,
  ...STREAM_OPERATIONS,
  ...RELAY_USAGE_OPERATIONS,
  ...NUDGES_OPERATIONS,
  ...RESOURCES_OPERATIONS,
  ...MACHINES_OPERATIONS,
  ...SPACES_SETTINGS_OPERATIONS,
  ...CHECKLIST_OPERATIONS,
  ...LINKS_OPERATIONS,
  ...CLI_OPERATIONS,
  ...CI_OPERATIONS,
  ...THREAD_OPERATIONS,
  ...PRESENCE_OPERATIONS,
  ...CLOUD_APPROVE_OPERATIONS,
  ...AGENT_THREADS_OPERATIONS,
  ...AGENT_SIGN_IN_OPERATIONS,
  ...RUNTIME_FIX_OPERATIONS,
  ...DRIVER_OPERATIONS,
  ...SETTINGS_OPERATIONS,
  ...NOTIFICATIONS_OPERATIONS,
  ...TELEMETRY_OPERATIONS,
  "spaces.usage",
  "spaces.reach",
  "spaces.windows",
  "stream.pip",
  "spaces.thumbnail",
  "spaces.chooseFiles",
  "spaces.droppedFiles",
  "spaces.sendFiles",
  "host.setUp",
  "host.joinWithCode",
  "host.action",
  "host.limits",
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
  /** The host's updater moved on (found, downloading, ready): ask `update.get` again. */
  | { type: "update.changed" }
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
  /** The host's copy of the getting-started checklist changed (`ops/checklist.ts`): refetch. */
  | { type: "checklist.changed" }
  | TeleportHostEvent
  /** The native app moved on while starting (the startup screen; payload: the new state). */
  | StartupHostEvent;

export type HostEventType = HostEvent["type"];

export const HOST_EVENTS = [
  "spaces.changed",
  "spaces.createProgress",
  "machines.changed",
  "settings.changed",
  "update.changed",
  "keyvault.changed",
  "session.signedIn",
  "session.signInFailed",
  "session.signedOut",
  "session.changed",
  "agents.changed",
  "checklist.changed",
  ...TELEPORT_EVENTS,
  ...STARTUP_EVENTS,
] as const satisfies readonly HostEventType[];

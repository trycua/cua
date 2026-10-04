// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The main window from the app core: the sidebar (`spaces::sidebar`), a
 * Space's detail with its toolbar and sections, the chrome around them
 * (`window::chrome`) and the Settings page (`settings::page`). The SwiftUI
 * app renders the same views.
 */
import { core } from "../core";
import type { DriveSyncInput } from "./persistent";
import type { RemoteWindow } from "./teleport";
import type { Location, Space, SpaceOs, SpaceStatus } from "./types";

export interface SidebarRow {
  id: string;
  name: string;
  /** The Space's OS icon id (the notch tiles' mark), before the name. */
  osIcon: string;
  status: SpaceStatus;
  statusText: string;
  detail: string;
  dim: boolean;
  selected: boolean;
  /** While it is being created: overall progress in thousandths (a ring). */
  progress: number | null;
  /** The percentage while it is being created, the error when it failed. */
  trailing: string | null;
  /** A Space one of your machines provides, nested under that machine's row. */
  nested?: boolean;
  /** The power button next to Delete, for a Space that turns off and on. */
  power?: PowerButton | null;
  /** Where a Space in your cloud runs ("AWS · us-west-2"), after the name. */
  place?: string;
}

/** The power button (the app core's `sidebar::PowerButton`). */
export interface PowerButton {
  /** SF Symbol: `power`. */
  symbol: string;
  /** Tooltip and label: "Suspend", "Resume", "Turn off", "Turn on" (while
   * it runs, "Suspending…" and the like). */
  help: string;
  /** A press turns it on (else off). */
  turnOn: boolean;
  enabled: boolean;
  /** An action runs: a spinner in its place. */
  busy: boolean;
}

export interface SidebarView {
  thisMachine: SidebarRow | null;
  sections: { title: string; rows: SidebarRow[] }[];
  selectedId: string | null;
  emptyText: string | null;
}

export function sidebar(spaces: Space[], query: string, selectedId: string | null): SidebarView {
  return core("sidebar.build", { spaces, query, selectedId });
}

export type DetailActionId = "teleport" | "pip" | "share" | "power" | "delete" | "open" | "cancel";

/** A fact's copy button (`sidebar::FactCopy`). */
export interface FactCopy {
  text: string;
  symbol: string;
  help: string;
  doneSymbol: string;
  doneHelp: string;
  confirmMs: number;
}

/** A fact's warning: an SF Symbol after the value and its tooltip. */
export interface FactWarning {
  symbol: string;
  help: string;
}

export interface SpaceDetailView {
  id: string;
  title: string;
  /** Status, Image, System, Kind, Architecture, Memory, Storage,
   * Identifier (each when known). */
  facts: { label: string; value: string; copy?: FactCopy; help?: string; warning?: FactWarning }[];
  isHost: boolean;
  showSections: boolean;
  canStream: boolean;
  previewText: string;
  /** While it is being created: overall progress in thousandths. */
  progress: number | null;
  /** Under the bar while it downloads: "4.2 of 23.9 GB · 85 MB/s · about 4 min". */
  progressText?: string | null;
  deleteLabel: string;
  removeOnly: boolean;
  actions: {
    id: DetailActionId;
    label: string;
    symbol: string | null;
    help: string;
    enabled: boolean;
    destructive: boolean;
    primary: boolean;
    /** Its action runs: a spinner in place of the icon. */
    busy?: boolean;
  }[];
  confirm: {
    title: string;
    message: string;
    confirmLabel: string;
    /** False for a Space in your cloud another device created. */
    confirmEnabled: boolean;
    /** Why the destructive button is disabled. */
    disabledReason?: string;
    /** For a Space in your cloud: "Remove from List" (it keeps running). */
    removeLabel?: string;
    cancelLabel: string;
  };
  sections: string[];
  /** A cloud Space refused for want of credit: one line and "Add credit"
   * (the website billing page). */
  creditNotice?: { text: string; button: string; url: string } | null;
  /** Why turning it off or on failed, shown inline. */
  powerError?: string | null;
}

/** Memory and storage use (the SDK's `Space.usage`), for Memory and Storage. */
export interface SpaceUsage {
  memoryUsed: number;
  memoryTotal: number;
  memoryLimited: boolean;
  diskUsed: number;
  diskTotal: number;
  diskLimited: boolean;
}

/** How often a visible detail refreshes its usage (the core's rate). */
export const USAGE_REFRESH_MS = 10_000;

/** The selected Space's detail (`spaces::sidebar::detail_live`). */
/** `hostArch`: this Mac's CPU architecture; a local Space of another one
 * warns on its Architecture (emulated). With `experiments`, what Settings,
 * Experiments hides is left out (Share while Sharing is off). */
export function spaceDetail(
  space: Space,
  usage?: SpaceUsage | null,
  hostArch?: string | null,
  experiments?: import("./experiments").Experiments | null,
): SpaceDetailView {
  return core("sidebar.detail", {
    space,
    usage: usage ?? null,
    hostArch: hostArch ?? null,
    experiments: experiments ?? null,
  });
}

export interface StreamSectionInput {
  /** The Space's windows, `null` while they load. */
  windows: RemoteWindow[] | null;
  failed?: boolean;
  /** The primary display, once the display list has been read. */
  display: { widthPx: number; heightPx: number } | null;
  os: SpaceOs;
  osName?: string | null;
  /** Row ids whose picture in picture is open. */
  open?: string[];
  query?: string;
}

export type StreamRowIcon =
  | { kind: "os"; id: string }
  | { kind: "app"; appName: string; appId: string; pid: number };

export interface StreamRow {
  /** "desktop" or the window's handle. */
  id: string;
  kind: "desktop" | "window";
  /** One line: "Desktop (1280×800)" or the window's title. */
  label: string;
  /** The full label, for the tooltip. */
  help: string;
  resolution: string | null;
  icon: StreamRowIcon;
  actions: { id: "pip"; symbol: string; help: string; active: boolean }[];
}

export interface StreamSection {
  rows: StreamRow[];
  statusText: string | null;
}

/** A Space's Stream section (`spaces::stream`): the rows both apps draw. */
export function streamSection(input: StreamSectionInput): StreamSection {
  return core("sidebar.streamSection", {
    input: { ...input, failed: input.failed ?? false, open: input.open ?? [], query: input.query ?? "" },
  });
}

/** A change to the open picture-in-picture panels (the shell reports it). */
export type PipEvent =
  | { type: "opened"; row: string }
  | { type: "closed"; row: string }
  | { type: "synced"; rows: string[] };

/** What a row's picture-in-picture button does. */
export type PipCommand = { type: "open"; row: string } | { type: "close"; row: string };

/** The open panels (row ids) after `event` (`spaces::stream::pip_reduce`). */
export function pipReduce(open: string[], event: PipEvent): string[] {
  return core("stream.pipReduce", { open, event });
}

/** What clicking `row`'s picture-in-picture button does. */
export function pipClick(open: string[], row: string): PipCommand {
  return core("stream.pipClick", { open, row });
}

export interface DetailCopy {
  streamLoading: string;
  streamEmpty: string;
  streamFailed: string;
  streamNoMatch: string;
  agentsLoading: string;
  agentsEmpty: string;
  agentsFailed: string;
  agentsNoMatch: string;
  dropCaption: string;
  sendFile: string;
  teleportApp: string;
  /** The one teleport icon (toolbar, drop well), and while a drag is over. */
  teleportSymbol: string;
  teleportSymbolActive: string;
}

let copyCache: DetailCopy | null = null;
/** The words of a Space's sections (Stream, Agents, Teleport). */
export function detailCopy(): DetailCopy {
  copyCache ??= core<DetailCopy>("sidebar.detailCopy");
  return copyCache;
}

export const deleteFailedText = (name: string, error: string): string =>
  core("sidebar.deleteFailedText", { name, error });

/** The power button of a Space, when it turns off and on. */
export const powerButton = (space: Space): PowerButton | null =>
  core("sidebar.powerButton", { space });

export interface MainChrome {
  title: string;
  newSpaceLabel: string;
  newSpaceShortcut: string;
  searchPlaceholder: string;
  keyvaultTitle: string;
  account: string;
  signInLabel: string | null;
  settingsLabel: string;
  settingsShortcut: string;
  emptyTitle: string;
  emptyAction: string;
  /** The sidebar's Volume page entry; null while Cua Volume is off. */
  volumeLabel?: string | null;
}

export function mainChrome(input: {
  identity?: string | null;
  cloudConfigured: boolean;
  canSignIn: boolean;
  /** Settings, Experiments (the Volume page only with Cua Volume on). */
  experiments?: import("./experiments").Experiments | null;
}): MainChrome {
  return core("window.chrome", { input: { ...input, identity: input.identity ?? null } });
}

export type SignInPhase =
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "waiting"; userCode?: string | null }
  | { kind: "failed"; message: string };

export interface AgentSettingsRow {
  agent: string;
  name: string;
  installed: boolean;
  configured: boolean;
  detail: string;
  skillsInstalled: number;
  skillsTotal: number;
  mcpConfig: string | null;
  skillsDir: string | null;
}

export interface SettingsInput {
  identity?: string | null;
  apiKeyClient?: string | null;
  signIn: SignInPhase;
  canSignOut: boolean;
  menuBar: boolean;
  defaultLocation: Location;
  locationLockedBy?: string | null;
  telemetry?: { enabled: boolean; lockedBy?: string | null } | null;
  agents?: AgentSettingsRow[] | null;
  agentsBusy: boolean;
  agentsPending: string[];
  /** The account's Cua Cloud billing (Settings, Billing), once read. */
  billing?: import("../native/fleet").BillingStatus | null;
  /** Launch at login, once the system was asked (none: no rows). */
  loginItem?: import("./loginItem").LoginItemInput | null;
  /** Settings, Experiments (what the page mentions follows them). */
  experiments?: import("./experiments").Experiments;
}

export interface SettingsRow {
  id: string;
  kind: "text" | "choice" | "note" | "error" | "field" | "secret" | "prompt" | "link" | "toggle";
  label: string;
  value: string | null;
  button: string | null;
  options: { id: string; label: string; active: boolean }[];
  enabled: boolean;
  help: string | null;
  linkLabel: string | null;
  linkUrl: string | null;
  /** A field's placeholder. */
  placeholder?: string | null;
}

export interface SettingsSection {
  id: string;
  title: string;
  button: string | null;
  buttonEnabled: boolean;
  buttonHelp: string | null;
  rows: SettingsRow[];
}

export interface SettingsPage {
  title: string;
  sections: SettingsSection[];
}

export function settingsPage(input: SettingsInput): SettingsPage {
  return core("settings.page", { input });
}

// ---------------------------------------------------------------- menu bar item

export type MenuItemId = "status" | "separator" | "open" | "new-space" | "settings" | "quit" | "volume-conflicts";

/** One item of the menu bar item's menu (`window::MenuItem`). */
export interface MenuItem {
  id: MenuItemId;
  label: string;
  /** "⌘,", "⌘Q". */
  shortcut: string | null;
  enabled: boolean;
}

/** What the menu bar item's menu shows (`window::MenuInput`). */
export interface MenuInput {
  /** The roster, with "This machine" (the notch's). */
  spaces: Space[];
  /** Live Keyvault sign-ins, when any. */
  keyvault: string | null;
  /** Cua Volume's `volume_sync_status`, when read. */
  sync: DriveSyncInput | null;
  /** `volume_storage`'s backend (`fs`, `s3`), when read. */
  backend?: string | null;
  /** Now (Unix ms): a bucket unheard of for a while reads Offline. */
  nowMs?: number;
  /** Settings, Experiments: the sync line only with Cua Volume on. */
  experiments?: import("./experiments").Experiments | null;
}

/** The menu bar item's menu: the Spaces the user can open (the notch's
 * count) with Cua Volume's sync state, its conflicts, then the actions. */
export function trayMenu(input: MenuInput): MenuItem[] {
  return core("window.menu", { input });
}

/** The Spaces the user can open: every Space but "This machine", which
 * counts only while it is shared and reachable (the notch tab's count). */
export function openableCount(spaces: readonly Space[]): number {
  return core("spaces.openableCount", { spaces });
}

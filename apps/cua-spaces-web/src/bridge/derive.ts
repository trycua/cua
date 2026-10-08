// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * View models from host data. Each function asks the app core first (the
 * same decisions the SwiftUI and Tauri apps make) and falls back to a small
 * TypeScript stand-in only when the wasm core is unavailable. The stand-ins
 * cover what a demo needs; they are not the product's behaviour, and the
 * parity flows run against the core path only.
 */

import type { CoreClient } from "./core";
import type { MachineAccessNotice } from "./contracts/devices";
import type { AgentRunStatus, PersistentAgent, SpaceAgentRun } from "./contracts/agents";
import type {
  HostFormView,
  HostPanelView,
  HostSetupChoice,
  HostStatus,
  MachineRow,
  SettingsInput,
  SettingsPage,
} from "./contracts/host";
import type {
  KeyvaultOverview,
  KvItem,
  KvListView,
  KvLock,
  KvPage,
  KvSelection,
  KvSidebar,
  VaultState,
  VaultView,
} from "./contracts/keyvault";
import type { CreateAction, CreatesState, PendingCreate, Space, SpaceOs, SpaceRow, SpaceStatus } from "./contracts/spaces";
import type { SessionSnapshot, SettingsSnapshot } from "./protocol";
import { notSharingOf, realLimits } from "./sharing";

/* ---- Spaces --------------------------------------------------------------- */

export function rowsToSpaces(core: CoreClient, rows: SpaceRow[], now: number): Space[] {
  return core.tryCall<Space[]>("spaces.rowsToSpaces", { rows, now }) ?? rows.map((r) => fallbackRowToSpace(r, now));
}

const SCENES: Record<SpaceOs, Space["scene"]> = {
  macos: "mac-desktop",
  windows: "windows-desktop",
  linux: "linux-terminal",
  unknown: "blank",
};

/** A Space name as shown: as the user typed it, trimmed (the core's `display_name`). */
function displayName(name: string): string {
  return name.trim() || name;
}

function fallbackRowToSpace(row: SpaceRow, now: number): Space {
  // Not reported: unknown, never guessed (the core's `row_to_space`).
  const os = row.os ?? "unknown";
  const off = row.powerState === "suspended" || row.powerState === "stopped";
  const status: SpaceStatus = row.reachable && !(row.power && off) ? "running" : "suspended";
  const added = row.addedAt ? Date.parse(row.addedAt) : NaN;
  return {
    id: row.id,
    name: displayName(row.name || row.id),
    os,
    status,
    detail: status === "running" ? "Running" : "Stopped",
    lastUsedAt: Number.isFinite(added) ? added : now,
    scene: SCENES[os],
    provider: row.provider,
    sdk: { features: row.features, spacesdVersion: row.spacesdVersion, reachable: row.reachable, error: row.error },
    osName: row.osName,
    osPrettyName: row.osPrettyName,
    image: row.image,
    kind: row.kind,
    arch: row.arch,
    host: row.host,
    hostName: row.hostName,
    power: row.power
      ? { control: row.power === "stop" ? "stop" : "suspend", off: status !== "running" }
      : undefined,
  };
}

export const NO_CREATES: CreatesState = { pending: [], deleting: [], powering: [] };

/** The create/power/delete state machine (`spaces::creating::reduce`). */
export function reduceCreates(core: CoreClient, state: CreatesState, action: CreateAction): CreatesState {
  return core.tryCall<CreatesState>("creates.reduce", { state, action }) ?? fallbackReduce(state, action);
}

/** Registry Spaces with the creates overlaid (`spaces::creating::compose`). */
export function composeCreates(core: CoreClient, spaces: Space[], state: CreatesState): Space[] {
  return core.tryCall<Space[]>("creates.compose", { spaces, state }) ?? fallbackCompose(spaces, state);
}

/** One line on what is wrong with an image reference, or null (`wizard::validate_image_ref`). */
export function validateImageRef(core: CoreClient, ref: string): string | null {
  return core.tryCall<string | null>("wizard.validateImageRef", { ref }) ?? null;
}

const PHASES = ["preparing", "pulling", "creating", "booting", "waiting_for_services", "connecting"];
const PHASE_LABEL: Record<string, string> = {
  preparing: "Preparing…",
  pulling: "Downloading…",
  creating: "Creating…",
  booting: "Starting…",
  waiting_for_services: "Starting…",
  connecting: "Connecting…",
};

function fallbackReduce(state: CreatesState, a: CreateAction): CreatesState {
  const pending = [...state.pending];
  const deleting = [...(state.deleting ?? [])];
  const powering = [...(state.powering ?? [])];
  const patch = (id: string, f: (p: PendingCreate) => PendingCreate) => {
    const i = pending.findIndex((p) => p.id === id);
    if (i >= 0) pending[i] = f(pending[i]!);
  };
  switch (a.type) {
    case "start":
      pending.push({
        id: a.id,
        name: a.name.trim() ? displayName(a.name) : a.os === "macos" ? "macOS Space" : a.os === "windows" ? "Windows Space" : "New Space",
        os: a.os,
        provider: a.provider,
        startedAt: a.now,
        phase: "preparing",
        pulled: false,
        permille: 0,
        image: a.image ?? null,
      });
      break;
    case "progress":
      patch(a.id, (p) => {
        const i = Math.max(0, PHASES.indexOf(a.phase));
        const permille = Math.round(((i + (a.fraction ?? 0)) / PHASES.length) * 1000);
        return { ...p, phase: a.phase, fraction: a.fraction, permille: Math.max(p.permille, Math.min(990, permille)) };
      });
      break;
    case "finish":
      patch(a.id, (p) => ({ ...p, spaceId: a.spaceId, permille: 1000 }));
      break;
    case "fail":
      patch(a.id, (p) => ({ ...p, error: a.error }));
      break;
    case "dismiss":
    case "cancel-done":
      return { pending: pending.filter((p) => p.id !== a.id), deleting, powering };
    case "cancel-start":
      patch(a.id, (p) => ({ ...p, cancelling: true }));
      break;
    case "delete-start":
      deleting.push({ id: a.id, startedAt: a.now, done: false });
      break;
    case "delete-done":
      return { pending, deleting: deleting.map((d) => (d.id === a.id ? { ...d, done: true } : d)), powering };
    case "delete-fail":
      return { pending, deleting: deleting.filter((d) => d.id !== a.id), powering };
    case "power-start":
      return { pending, deleting, powering: [...powering.filter((p) => p.id !== a.id), { id: a.id, on: a.on, startedAt: a.now, done: false }] };
    case "power-done":
      return { pending, deleting, powering: powering.filter((p) => p.id !== a.id) };
    case "power-fail":
      return { pending, deleting, powering: powering.map((p) => (p.id === a.id ? { ...p, error: a.error } : p)) };
    default:
      break;
  }
  return { pending, deleting, powering };
}

function fallbackCompose(spaces: Space[], state: CreatesState): Space[] {
  const out: Space[] = [];
  for (const s of spaces) {
    const d = state.deleting?.find((x) => x.id === s.id);
    if (d?.done) continue;
    if (d) {
      out.push({ ...s, status: "deleting", detail: "Deleting…" });
      continue;
    }
    const p = state.powering?.find((x) => x.id === s.id);
    if (p && !p.done && !p.error) {
      out.push({
        ...s,
        detail: p.on ? "Starting…" : "Stopping…",
        power: s.power ? { ...s.power, turningOn: p.on } : { control: "suspend", off: !p.on, turningOn: p.on },
      });
      continue;
    }
    out.push(p?.error && s.power ? { ...s, power: { ...s.power, error: p.error } } : s);
  }
  const listed = new Set(spaces.map((s) => s.id));
  for (const p of state.pending) {
    if (p.spaceId && listed.has(p.spaceId)) continue;
    out.push({
      id: p.id,
      name: p.name,
      os: p.os,
      status: "provisioning",
      detail: p.error ? "Failed" : (PHASE_LABEL[p.phase] ?? "Creating…"),
      lastUsedAt: p.startedAt,
      startedAt: p.startedAt,
      scene: SCENES[p.os],
      provider: p.provider === "relay" ? "relay" : p.provider,
      progress: {
        phase: p.phase,
        permille: p.permille,
        label: p.error ? "Failed" : (PHASE_LABEL[p.phase] ?? "Creating…"),
        error: p.error ?? undefined,
        cancellable: !p.error && !p.cancelling,
        cancelling: p.cancelling,
      },
    });
  }
  return out;
}

/** Whether one of your machines shares its desktop, when it said so
 * (`spaces.sharesDesktop`): `false` for a machine set up to provide Spaces
 * only. It refuses its desktop and its processes (windows, usage, agent
 * runs), so nothing asks it for them; null: not known, or not one of your
 * machines. */
export function sharesDesktop(core: CoreClient | undefined, space: Space): boolean | null {
  return core?.tryCall<boolean | null>("spaces.sharesDesktop", { space }) ?? null;
}

/* ---- Machines -------------------------------------------------------------- */

export interface Machine {
  id: string;
  name: string;
  /** `macos`, `linux`, `windows`, or what the host reported (may be empty). */
  os: string;
  /** `local` (this machine), `relay` or `direct`. */
  via: string;
  /** How other devices reach it: `relay` or `direct`; null for this machine
   * before host setup. */
  connection?: "relay" | "direct" | null;
  /** The relay sees it connected, or the app reached it. */
  online: boolean;
  /** Online, but its owner stopped sharing it: why, in one line. Calls to it
   * are refused, so New Space lists it as not sharing. */
  notSharing?: string;
  /** The machine this UI runs on. */
  current: boolean;
  model?: string;
  /** One line in the host's words (sharing summary, device state). */
  detail?: string;
  /** Last session at the relay, Unix seconds. */
  lastSeen?: number;
  /** This machine's CPU architecture (`aarch64`, `x86_64`), when the host says. */
  arch?: string;
  /** Spaces running on it, in roster order. */
  spaceIds: string[];
  limits: MachineRow["limits"];
  /** This machine only: the core's "This machine" page, when the host
   * reported its status. */
  panel?: HostPanelView;
  /** This machine only: what a running setup or Sign In waits for
   * (finishing the sign-in in the browser), when the host says. */
  hostProgress?: string;
  /** One of the account's devices that is none of your machines: listed,
   * but New Space never runs on it. */
  device?: boolean;
  /** This machine only: signed in, but it cannot open the account's
   * machines (not enrolled): each machine's detail says why. */
  accessNotice?: MachineAccessNotice;
}

/** Which machine a Space runs on: its relay host, the machine itself for
 * its own desktop (`relay:<machine>`), or this machine for a local Space;
 * undefined for cloud Spaces. */
export function machineIdOf(space: Pick<Space, "id" | "host" | "provider">, machines: Pick<MachineRow, "id" | "current">[]): string | undefined {
  if (space.host) return space.host;
  if (space.provider === "local") return machines.find((m) => m.current)?.id;
  if (space.provider === "relay" && space.id.startsWith("relay:")) {
    const own = space.id.slice("relay:".length);
    if (machines.some((m) => m.id === own)) return own;
  }
  return undefined;
}

function connectionOf(m: MachineRow): Machine["connection"] {
  if (m.device) return null;
  if (m.current) return m.host?.configured ? (m.host.mode === "direct" ? "direct" : "relay") : null;
  return m.via === "direct" ? "direct" : m.via === "relay" ? "relay" : null;
}

/** What `machines.merge` reads (`machines::MachinesInput`). */
export interface MachinesInput {
  machines: { id: string; name: string; online: boolean; presence: boolean | null; hostname: string | null; os: string; current: boolean }[];
  devices: { id: string; name: string; platform: string; lastSeen: number | null; current: boolean; state: string }[];
  /** Unix seconds. */
  now: number;
}

/** One Machines row as the core lists it (`machines::MachineOut`). */
export interface MergedMachine {
  id: string;
  name: string;
  online: boolean;
  device: string | null;
  deviceOnly: boolean;
  current: boolean;
}

const nameKey = (name: string) => name.trim().toLowerCase().replace(/\.$/, "");

/** Without the core: a device merges into the machine with its id, or
 * the one whose cua-spacesd reported its name as the hostname. */
function fallbackMerge({ machines, devices }: MachinesInput): MergedMachine[] {
  const out: MergedMachine[] = machines.map((m) => ({
    id: m.id,
    name: m.name,
    online: m.online || m.presence === true,
    device: null,
    deviceOnly: false,
    current: m.current,
  }));
  for (const d of devices) {
    const i = machines.findIndex((m, i) => !out[i]!.device && (m.id === d.id || (m.current && d.current) || (m.hostname && nameKey(m.hostname) === nameKey(d.name))));
    if (i >= 0) out[i]!.device = d.id;
    else if (!d.current && d.state !== "revoked") out.push({ id: d.id, name: d.name || d.id, online: false, device: d.id, deviceOnly: true, current: false });
  }
  return out;
}

/** Each computer once (`machines.merge`): a machine on the relay and the
 * enrolled device on it are one row, with the machine's id and name. */
export function mergeMachines(core: CoreClient | undefined, input: MachinesInput): MergedMachine[] {
  return core?.tryCall<MergedMachine[]>("machines.merge", { input }) ?? fallbackMerge(input);
}

/** The core's input for these host rows (`device` rows are devices). */
export function machinesInput(rows: MachineRow[], now: number): MachinesInput {
  return {
    machines: rows
      .filter((r) => !r.device)
      .map((m) => ({
        id: m.id,
        name: m.name,
        online: m.online,
        presence: m.presence ?? null,
        hostname: m.hostname ?? null,
        os: m.os,
        current: Boolean(m.current),
      })),
    devices: rows
      .filter((r) => r.device)
      .map((d) => ({ id: d.id, name: d.name, platform: d.os, lastSeen: d.lastSeen ?? null, current: Boolean(d.current), state: d.deviceState ?? "" })),
    now,
  };
}

/** The host's machine and device rows, each computer once
 * (`mergeMachines`). The device adds its detail and last session; a
 * device that is none of the machines stays a row of its own. */
export function mergeMachineRows(rows: MachineRow[], core?: CoreClient, nowSecs = Math.floor(Date.now() / 1000)): MachineRow[] {
  if (!rows.some((r) => r.device || r.presence != null)) return rows;
  const byId = new Map(rows.filter((r) => !r.device).map((r) => [r.id, r] as const));
  const devices = new Map(rows.filter((r) => r.device).map((r) => [r.id, r] as const));
  return mergeMachines(core, machinesInput(rows, nowSecs)).map((m) => {
    const dev = m.device ? devices.get(m.device) : undefined;
    if (m.deviceOnly) return { ...dev!, online: m.online, device: true };
    const row = byId.get(m.id)!;
    return { ...row, name: m.name, online: m.online, detail: row.detail ?? dev?.detail, lastSeen: row.lastSeen ?? dev?.lastSeen };
  });
}

export function toMachines(rows: MachineRow[], spaces: Space[], core?: CoreClient, nowSecs?: number): Machine[] {
  rows = mergeMachineRows(rows, core, nowSecs);
  return rows.map((m) => {
    const notSharing = notSharingOf(m.limits, m.online);
    return {
      id: m.id,
      name: m.name,
      os: m.os,
      via: m.via,
      connection: connectionOf(m),
      online: m.online,
      current: Boolean(m.current),
      model: m.model,
      // The host's own words for it, but what the page says about a
      // machine that stopped sharing is the reason, not "unreachable".
      detail: notSharing ?? m.detail ?? (m.current && m.host && core ? hostPanel(core, m.host).summary : undefined),
      lastSeen: m.lastSeen ?? undefined,
      arch: m.arch,
      limits: realLimits(m.limits),
      spaceIds: spaces.filter((s) => machineIdOf(s, rows) === m.id).map((s) => s.id),
      panel: m.current && m.host && core ? hostPanel(core, m.host) : undefined,
      ...(m.current && m.host?.progress ? { hostProgress: m.host.progress } : {}),
      ...(notSharing ? { notSharing } : {}),
      ...(m.device ? { device: true } : {}),
      ...(m.current && m.accessNotice ? { accessNotice: m.accessNotice } : {}),
    };
  });
}

/* ---- This machine as a host ---------------------------------------------- */

/** The host status in the core's `HostState` shape (as the Tauri app's
 * `model/host.ts` maps it). `progress` is the shell's, not the core's: the
 * page draws it under the panel itself. */
export function hostState(status: HostStatus) {
  return {
    configured: status.configured,
    mode: status.mode ?? null,
    relayUrl: status.relayUrl ?? null,
    directUrl: status.directUrl ?? null,
    name: status.name ?? null,
    sharing: status.sharing,
    serviceInstalled: status.service.installed,
    serviceRunning: status.service.running,
    serviceKind: status.service.kind,
    online: status.online ?? null,
    clients: status.clients.map((c) => ({ id: c.id, email: c.email ?? null, name: c.name ?? null, streams: c.streams ?? null })),
    permissions: status.permissions.map((p) => ({
      id: p.id,
      title: p.label,
      settingsUrl: p.settingsUrl ?? null,
      instructions: p.instructions ?? null,
      granted: Boolean(p.granted),
    })),
    error: status.error ?? null,
    recentAccess: status.recentAccess ?? [],
    accessLogError: status.accessLogError ?? null,
    shareDesktop: status.shareDesktop ?? true,
    provideSpaces: status.provideSpaces ?? false,
    maxSpaces: status.maxSpaces ?? 0,
    maxMacosVms: status.maxMacosVms ?? 0,
    providedSpaces: status.providedSpaces ?? [],
    spacesAudit: status.spacesAudit ?? [],
    spacesAuditError: status.spacesAuditError ?? null,
    pausedSignedOut: status.pausedSignedOut ?? false,
    owner: status.owner ?? null,
    ownerEmail: status.ownerEmail ?? null,
    account: status.account ?? null,
  };
}

/** The "This machine" page (`host.panel`). */
export function hostPanel(core: CoreClient, status: HostStatus | null): HostPanelView {
  return hostPanelFromState(core, status ? hostState(status) : null, status);
}

/** The page for the core's own host state (`host::HostState`); `status` is
 * what the stand-in draws without the core. */
export function hostPanelFromState(core: CoreClient, state: object | null, status: HostStatus | null = null): HostPanelView {
  return core.tryCall<HostPanelView>("host.panel", { state }) ?? fallbackPanel(status);
}

function fallbackPanel(s: HostStatus | null): HostPanelView {
  const base: HostPanelView = { title: "This machine", summary: "Checking…", configured: false, facts: [], clients: [], permissions: [], openSettingsLabel: "Open Settings", actions: [] };
  if (!s) return base;
  if (!s.configured) {
    return { ...base, summary: "Other devices can’t reach this machine yet.", intro: FALLBACK_INTRO, setupChoices: FALLBACK_CHOICES };
  }
  const via = s.mode === "direct" ? "Direct" : "Relay";
  const summary = !s.service.running ? "Host service stopped" : !s.sharing ? "Not sharing" : `Sharing · ${via}`;
  const access = s.mode === "direct" ? `Direct at ${s.directUrl || "this machine’s address"}` : s.relayUrl ? `Relay ${s.relayUrl}` : "Relay";
  return {
    ...base,
    summary,
    configured: true,
    facts: [
      { label: "Name", value: s.name || "This machine" },
      { label: "Access", value: access },
      { label: "Service", value: `${s.service.kind} · ${s.service.running ? "running" : s.service.installed ? "stopped" : "not installed"}` },
    ],
    clientsTitle: "Connected now",
    clients: s.clients.map((c) => c.email ?? c.name ?? c.id),
    clientsEmpty: s.clients.length ? null : "Nobody",
  };
}

const FALLBACK_INTRO =
  "This is the computer you are using now. Set it up for access to reach it and its Spaces from your other devices and your agents, over the Cua relay with no port forwarding.";
const FALLBACK_CHOICES: HostSetupChoice[] = [
  { id: "desktop", label: "Share this desktop", buttonLabel: "Set Up…" },
  { id: "spare", label: "Use as a spare machine for Spaces", buttonLabel: "Set Up…" },
];

/** What "Add a machine" explains: the core's words for host setup, as the
 * other machine will show them. Read-only; nothing here sets anything up. */
export interface HostSetupGuide {
  /** What setting a machine up does (`host::HOST_INTRO`). */
  intro: string;
  /** The ways to set it up (`host::setup_choices`). */
  choices: HostSetupChoice[];
  /** The setup form with Advanced open (`host.formView`), when the core is loaded. */
  form: HostFormView | null;
}

export function hostSetupGuide(core: CoreClient): HostSetupGuide {
  const panel = hostPanel(core, { configured: false, sharing: false, service: { installed: false, running: false, kind: "process" }, clients: [], permissions: [] });
  const initial = core.tryCall<unknown>("host.formInitial");
  const state = initial === undefined ? undefined : core.tryCall<unknown>("host.formReduce", { state: initial, action: { type: "toggle-advanced" } });
  const form = state === undefined ? null : (core.tryCall<HostFormView>("host.formView", { state, identity: null }) ?? null);
  return { intro: panel.intro ?? FALLBACK_INTRO, choices: panel.setupChoices?.length ? panel.setupChoices : FALLBACK_CHOICES, form };
}

/* ---- Settings ----------------------------------------------------------- */

/** The Settings page as the app core lays it out, or null without the core. */
export function settingsPage(
  core: CoreClient,
  settings: SettingsSnapshot,
  session: SessionSnapshot | undefined,
  signIn: SettingsInput["signIn"],
): SettingsPage | null {
  const loc = settings.values.defaultLocation;
  const input: SettingsInput = {
    identity: session?.fleet.identity ?? null,
    apiKeyClient: session?.fleet.authMode === "client-credentials" ? (session.fleet.clientId ?? null) : null,
    signIn: signIn ?? { kind: "idle" },
    canSignOut: session?.fleet.authMode === "user",
    menuBar: settings.values.menuBar,
    defaultLocation: loc === "cloud" ? "cloud" : loc === "local" ? "local" : loc.startsWith("host:") ? "host" : "yours",
    locationLockedBy: settings.defaultLocation?.source === "env" ? (settings.defaultLocation.env ?? null) : null,
    telemetry: settings.telemetry
      ? {
          enabled: settings.telemetry.enabled,
          lockedBy: ["do_not_track", "env", "legacy_env", "ci"].includes(settings.telemetry.sourceKind)
            ? settings.telemetry.source
            : null,
        }
      : null,
    // The host's own settings (the SwiftUI app's): the core lays out the
    // Runtimes section, auto-connect and the Keyvault's auto-wipe as there.
    keyvaultAutoWipe: settings.hostSettings?.keyvaultAutoWipe ?? null,
    autoConnect: settings.hostSettings?.autoConnect ?? null,
    lumeSource: settings.hostSettings?.lumeSource ?? null,
    linuxSource: settings.hostSettings?.linuxSource ?? null,
  };
  return core.tryCall<SettingsPage>("settings.page", { input }) ?? null;
}

/* ---- Keyvault ----------------------------------------------------------- */

/** Items grouped by the app they came from ("Google Chrome", "Slack"). */
export interface KeyvaultAppGroup {
  /** The provider id (`chrome`): the app's icon and the `app` selector. */
  key: string;
  app: string;
  items: KvItem[];
  /** Locked: every item asks for each use. Unlocked: allowed unattended. */
  lock: KvLock;
}

/** A plain grouping, for hosts without the core (the core's `vault` view
 * is the one to draw when present). */
export function groupByApp(items: KvItem[]): KeyvaultAppGroup[] {
  const groups = new Map<string, { app: string; items: KvItem[] }>();
  for (const item of items) {
    const g = groups.get(item.provider_id) ?? { app: item.app_display || item.provider_id, items: [] };
    g.items.push(item);
    groups.set(item.provider_id, g);
  }
  return [...groups.entries()]
    .sort(([, a], [, b]) => a.app.localeCompare(b.app))
    .map(([key, g]) => {
      const open = g.items.filter((i) => i.policy.unattended).length;
      return { key, app: g.app, items: g.items, lock: open === 0 ? "locked" : open === g.items.length ? "unlocked" : "mixed" };
    });
}

export const INITIAL_VAULT_STATE: VaultState = { query: "", selected: [], expanded: [], app: null };

/** The app core's Keyvault views; null without the core. */
export interface KeyvaultViews {
  /** Page chrome: availability, setup/unlock form, kill switch, labels. */
  page: KvPage;
  /** Categories (All, Waiting, Access, Recent) and apps. */
  sidebar: KvSidebar;
  /** The vault list grouped by app, then site, with locks and selection. */
  vault: VaultView;
}

export function keyvaultViews(
  core: CoreClient,
  overview: KeyvaultOverview,
  now: number,
  state: VaultState = INITIAL_VAULT_STATE,
): KeyvaultViews | null {
  if (core.status !== "ready") return null;
  return {
    page: core.call<KvPage>("keyvault.page", { overview, now }),
    sidebar: core.call<KvSidebar>("keyvault.sidebar", { overview, now }),
    vault: core.call<VaultView>("keyvault.vaultView", { overview, state, now }),
  };
}

/** The pane for a sidebar selection (`vault: true` means draw the vault view). */
export function keyvaultPane(core: CoreClient, overview: KeyvaultOverview, selection: KvSelection, now: number): KvListView | null {
  return core.tryCall<KvListView>("keyvault.list", { overview, selection, now }) ?? null;
}

/* ---- Agents --------------------------------------------------------------- */

/** One persistent agent as the core's Agents page draws it (`agents.pageView`'s rows). */
export interface AgentRowView {
  name: string;
  /** `Claude Code in local:design-review`. */
  detail: string;
  /** `Running`, `Idle` or `Paused`. */
  state: string;
  /** `Pause` or `Resume`. */
  actionLabel: string;
  selected: boolean;
}

const HARNESS_NAMES: Record<string, string> = {
  "claude-code": "Claude Code",
  "openai-codex": "Codex",
  codex: "Codex",
  hermes: "Hermes",
  openclaw: "OpenClaw",
  "gemini-cli": "Gemini CLI",
  "google-antigravity": "Antigravity",
  goose: "Goose",
  opencode: "OpenCode",
  pi: "Pi",
};

/** The Agents page's own state (`agents.pageInitial`). */
export function agentsPageInitial(core: CoreClient): object {
  return core.tryCall<object>("agents.pageInitial", {}) ?? {};
}

/** The Agents page as the core draws it (`agents.pageView`); null without the core. */
export function agentsPageView(core: CoreClient, input: object, state: object, nowMs: number): { rows: AgentRowView[] } | null {
  return core.tryCall<{ rows: AgentRowView[] }>("agents.pageView", { input, state, nowMs }) ?? null;
}

/** The core's rows for `agents` (sorted by name), or a stand-in without it. */
export function agentRows(core: CoreClient, agents: PersistentAgent[], now: number): AgentRowView[] {
  const view = agentsPageView(core, { agents }, agentsPageInitial(core), now);
  if (view) return view.rows;
  return [...agents]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((a) => ({
      name: a.name,
      detail: `${HARNESS_NAMES[a.harness] ?? a.harness} in ${a.space}`,
      state: a.paused ? "Paused" : a.runId ? "Running" : "Idle",
      actionLabel: a.paused ? "Resume" : "Pause",
      selected: false,
    }));
}

/** A harness's display name (`agents.name`: `claude-code` -> `Claude Code`). */
export function harnessName(core: CoreClient, harness: string): string {
  return core.tryCall<string>("agents.name", { agent: harness }) ?? HARNESS_NAMES[harness] ?? (harness || "Unknown agent");
}

const ATTENTION: Record<AgentRunStatus, number> = { failed: 0, crashed: 1, idle: 2, running: 3, unknown: 4 };

/** Runs in reading order: whatever needs a human first, then newest (`agents.order`). */
export function orderRuns<T extends SpaceAgentRun>(core: CoreClient, runs: T[]): T[] {
  const ordered = core.tryCall<SpaceAgentRun[]>("agents.order", { runs });
  if (ordered && ordered.length === runs.length) {
    // The core returns its own records; keep ours (they carry the Space) in its order.
    const byId = new Map(runs.map((r) => [r.runId, r]));
    const out = ordered.map((r) => byId.get(r.runId)).filter((r): r is T => r !== undefined);
    if (out.length === runs.length) return out;
  }
  return [...runs].sort((a, b) => ATTENTION[a.status] - ATTENTION[b.status] || (b.createdAt ?? 0) - (a.createdAt ?? 0));
}

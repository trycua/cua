// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The native hosts: the SwiftUI app's WKWebView (`WebUIBridge.swift`) and,
 * over its own transport, the Electron shell (`src/bridge/` in
 * apps/cua-spaces-desktop), which answers the same methods. Their methods
 * and envelope (see `../webkit-protocol.ts`), mapped onto the bridge's
 * operations. The host returns the app core's view models; this adapter
 * turns them back into the wire shapes `protocol.ts` names, so the store
 * derives the same way for every host.
 *
 * Every operation reaches the host (`../coverage.ts` lists how). The
 * `agents.*` methods keep the operation's name and arguments, and answer the
 * contract's records (`agents.events` is the `agent_events` result as is);
 * without the daemon they fail with `unsupported`. New Space runs the
 * page's wizard on the host's env (`spaces.createOptions`) and creates
 * through the app's own create (`spaces.create`, progress as
 * `spaces.createProgress`); the host's menu items ask for the wizard with
 * `spaces.newRequested`. By design: a passphrase never
 * crosses the bridge, so `keyvault.unlock` with one fails with `native_only`.
 * The first run is this page's on both hosts (`onboarding.get`,
 * `onboarding.complete`). Appearance and the hotkey are page-side; an appearance change sets
 * the window's background (`window.setBackgroundColor`). The page tells the
 * window where it drags (`window.setDragRegions`, `../drag-regions.ts`), and
 * where its Space tiles are for a real window dragged onto one
 * (`window.setDropTargets`, `../window-drop.ts`).
 */

import { Emitter, HostError, type DataAdapter } from "../adapter";
import { hostOs, THIS_MACHINE, type HostOs } from "../host-os";
import { electronTransport, webkitTransport, type HostTransport } from "../transport";
import type { AgentEventsPage, AgentMessageSent, AgentSetupRow, AgentStarted, ConnectionReport, ConnectResult, PersistentAgent, SpaceAgentRun } from "../contracts/agents";
import type { CancelOutcome, HostActionId, HostStatus, MachinePageRow, MachineRow, MachinesView, SpacePowerReport } from "../contracts/host";
import type { HostWindow } from "../detect";
import {
  DEFAULT_SETTINGS,
  type HostEvent,
  type OpArgs,
  type OpName,
  type OpResult,
  type SessionSnapshot,
  type SettingsSnapshot,
  type SettingsValues,
} from "../protocol";
import type { KeyvaultOverview, KvGrant, KvPending } from "../contracts/keyvault";
import type { CreateProgress } from "../contracts/host";
import { syncDragRegions } from "../drag-regions";
import { setWindowDropTarget, syncDropTargets, WINDOW_DROP_EVENT, windowDropTargetOf } from "../window-drop";
import { requestNewSpace } from "../new-space";
import { STOPPED_SHARING_ERROR, stoppedSharingLine, withNotSharing } from "../sharing";
import { teleportProgressFromWk, webkitPageOps } from "../ops/webkit-pages";
import { webkitAgentDetailOps } from "../ops/agent-detail";
import { webkitThreadOps } from "../ops/thread";
import { webkitAgentKeysOps } from "../ops/agent-keys";
import { webkitKeyvaultManageOps } from "../ops/keyvault-manage";
import { webkitKeyvaultSetupOps } from "../ops/keyvault-setup";
import { webkitSettingsOps } from "../ops/settings";
import { startupFromHost, webkitStartupOps } from "../ops/startup";
import { webkitTelemetryNoticeOps } from "../ops/telemetry";
import { unsupportedStreamOps, type StreamTicket } from "../ops/stream";
import type { RelayUsage } from "../ops/relay-usage";
import type { RiskyRequest } from "../ops/nudges";
import type { ResourcesSnapshot } from "../ops/resources";
import type { HostChecklistState } from "../ops/checklist";
import type { LinkCreate, ShareLink } from "../ops/links";
import { unsupportedPresenceOps, type PresenceJoinInfo } from "../ops/presence";
import { cloudApproveWebkitOps } from "../ops/cloud-approve";
import { unsupportedRuntimeFixOps } from "../ops/runtime-fix";
import type { CliInstallPlan } from "../contracts/first-run";
import type { BulkResult } from "../contracts/spaces-page";
import type { CreatingElsewhere, ListStatus, SpaceAccess, SpaceRow, SpaceSharingFacts } from "../contracts/spaces";
import {
  isWebkitResponse,
  WEBKIT_EVENT,
  WEBKIT_OPEN_CHECKLIST_EVENT,
  WEBKIT_OPEN_SETTINGS_EVENT,
  type HostMethod,
  type WebkitEventDetail,
  type WebkitRequest,
  type WkHostState,
  type WkKeyvault,
  type WkMachines,
  type WkSession,
  type WkSettings,
  type WkSettingsRow,
  type WkSpace,
  type WkSpaces,
  type WkOnboarding,
} from "../webkit-protocol";

export interface WebkitAdapterOptions {
  /** How long a request may wait for its answer (ms). */
  timeoutMs?: number;
  /** Which native host: the SwiftUI app (default) or the Electron shell. */
  host?: "webkit" | "electron";
}

/* ---- Results -> wire shapes -------------------------------------------------- */

const opt = <T>(v: T | null | undefined): T | undefined => (v === null ? undefined : v);

/** An `AppSpace` back to the registry row the core maps (`model::SpaceRow`). */
export function spaceToRow(s: WkSpace): SpaceRow {
  // This Mac's own desktop (`this-mac`; status `local` once it is set up
  // for access): it runs here, and is never created.
  const running = s.status === "local";
  const here = running || s.id === "this-mac";
  const off = s.power ? s.power.off : s.status === "suspended";
  const powerState = s.power ? (off ? (s.power.control === "stop" ? "stopped" : "suspended") : "running") : off ? "suspended" : undefined;
  const added = s.startedAt ?? s.lastUsedAt;
  return {
    id: s.id,
    name: s.name,
    provider: s.provider ?? (here ? "local" : "direct"),
    spacesdVersion: s.sdk?.spacesdVersion ?? "",
    features: s.sdk?.features ?? [],
    addedAt: added ? new Date(added).toISOString() : undefined,
    os: s.os,
    osName: opt(s.osName),
    osPrettyName: opt(s.osPrettyName),
    image: opt(s.image),
    imageDigest: opt(s.imageDigest),
    kind: opt(s.kind),
    arch: opt(s.arch),
    reachable: running || (s.sdk?.reachable ?? !off),
    error: opt(s.sdk?.error),
    host: opt(s.host),
    hostName: opt(s.hostName),
    power: s.power?.control,
    powerState: running ? "running" : powerState,
    stopReason: off ? opt(s.power?.stopReason) : undefined,
    cloud: opt(s.cloud),
    cloudPlace: opt(s.cloudPlace),
    cloudDelete: opt(s.cloudDelete),
    sharing: s.access ? sharingOf(s.access) : undefined,
    hostProgress: s.id.startsWith("pending:") ? (s.progress ?? undefined) : undefined,
  };
}

/** The sharing facts back from the core's `SpaceAccess` on an `AppSpace`,
 * so the core draws the same access again from the row. */
export function sharingOf(a: SpaceAccess): SpaceSharingFacts | undefined {
  if (a.kind === "mine" && a.people.length === 0) return undefined;
  const people = (role: "editor" | "viewer") => a.people.filter((p) => !p.team && p.role === role).map((p) => p.who);
  const teams = a.people
    .filter((p) => p.team)
    .map((p) => ({ key: p.who.replace(/^team:/, ""), name: p.name ?? p.who.replace(/^team:/, ""), role: p.role }));
  const owner = a.role === "owner";
  return {
    ownerId: a.ownerEmail ?? a.ownerName ?? "",
    ownerEmail: a.ownerEmail ?? undefined,
    ownerName: a.ownerName ?? undefined,
    role: a.role,
    // A team Space you made names its team among the people; one you reach
    // through a team names it here.
    team: a.kind === "team" && !owner ? (a.teamName ?? undefined) : undefined,
    teamName: a.kind === "team" && !owner ? (a.teamName ?? undefined) : undefined,
    editors: people("editor"),
    viewers: people("viewer"),
    teams,
  };
}

/** This Mac, the machines that provide the listed Spaces (and those that
 * share their own desktop, `relay:<machine>`), then the account's other
 * devices, marked `device`. Each relay machine carries what the relay
 * says of it (`presence`) and the hostname its cua-spacesd reported, so
 * the store lists a machine and the device on it once
 * (`mergeMachineRows`). */
export function toMachineRows(spaces: WkSpaces, machines: WkMachines | null, os: HostOs = "macos"): MachineRow[] {
  if (machines?.rows?.length) return fromPageRows(spaces, machines, machines.rows);
  const self: MachineRow = { id: "this-mac", name: THIS_MACHINE[os], via: "local", online: true, os, current: true, limits: [] };
  if (machines?.thisMachine?.detail) self.detail = machines.thisMachine.detail;
  if (machines?.host) self.host = toHostStatus(machines.host);
  if (machines?.accessNotice) self.accessNotice = machines.accessNotice;
  const rows: MachineRow[] = [self];
  const byId = new Map(rows.map((r) => [r.id, r] as const));
  // This computer is also on the relay under its own machine id once it is set
  // up for access: that is the same row, not a second machine.
  const hostMachineId = machines?.host?.machineId || undefined;
  if (hostMachineId) byId.set(hostMachineId, self);
  const add = (row: MachineRow) => {
    byId.set(row.id, row);
    rows.push(row);
  };
  for (const { space } of spaces.spaces) {
    // A machine's own desktop: `relay:<machine>` with no host above it.
    const own = space.provider === "relay" && !space.host && space.id.startsWith("relay:") ? space.id.slice(6) : undefined;
    // A Space registered on the relay (`space-…`) is a Space, not a machine.
    if (own?.startsWith("space-")) continue;
    if (own && !own.includes("/")) {
      // The list's connect probe was refused by a machine that stopped
      // sharing: it is connected (that is how it refused), not offline.
      const stopped = STOPPED_SHARING_ERROR.test(space.sdk?.error ?? "");
      const online = space.status === "running" || space.status === "local" || stopped;
      const limits = stopped ? withNotSharing([], stoppedSharingLine(space.name)) : [];
      const known = byId.get(own);
      // This computer keeps its own name and system ("This computer", Linux).
      if (known === self) continue;
      if (known) Object.assign(known, { name: space.name, os: space.os, online, limits });
      else add({ id: own, name: space.name, via: "relay", online, os: space.os, detail: space.detail || undefined, limits });
      continue;
    }
    if (!space.host || byId.has(space.host)) continue;
    add({ id: space.host, name: space.hostName ?? space.host, via: "relay", online: true, os: "", limits: [] });
  }
  for (const [id, hostname] of Object.entries(machines?.hostnames ?? {})) {
    const row = byId.get(id);
    if (row && hostname) row.hostname = hostname;
  }
  for (const [id, connected] of Object.entries(machines?.presence ?? {})) {
    const row = byId.get(id);
    if (row && !row.current) row.presence = connected;
  }
  for (const d of machines?.devices?.rows ?? []) {
    rows.push({
      id: d.id,
      name: d.name,
      via: "relay",
      online: false,
      os: d.platform.toLowerCase(),
      limits: [],
      current: d.current || undefined,
      device: true,
      deviceState: machines?.deviceStates?.[d.id],
      detail: d.detail || undefined,
      lastSeen: d.lastSeen ?? undefined,
    });
  }
  return rows;
}

/** A host action as the bridge names it: the hosts encode the core's enum
 * by its case (`provideSpaces`), the bridge by its wire word
 * (`provide-spaces`). */
const actionId = (a: string): HostActionId => a.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`) as HostActionId;

/** The host's Machines view with its switches' actions as the bridge names them. */
export function fromHostView(v: MachinesView): MachinesView {
  return {
    ...v,
    machines: v.machines.map((m) => ({
      ...m,
      toggles: m.toggles.map((t) => ({ ...t, actions: t.actions.map(actionId) })),
      noticeAction: m.noticeAction ? { ...m.noticeAction, id: actionId(m.noticeAction.id) } : m.noticeAction,
      issues: m.issues?.map((i) => (i.action?.hostAction ? { ...i, action: { ...i.action, hostAction: actionId(i.action.hostAction) } } : i)),
    })),
  };
}

/** The host's rows from the core (`machines.pageRows`, as the SwiftUI
 * Machines page lists them), with what the page draws beside them: this
 * machine's host status and summary, a device's state and detail, and why
 * a machine that stopped sharing is not reachable. */
function fromPageRows(spaces: WkSpaces, machines: WkMachines, rows: MachinePageRow[]): MachineRow[] {
  const devices = new Map((machines.devices?.rows ?? []).map((d) => [d.id, d] as const));
  const spaceById = new Map(spaces.spaces.map(({ space }) => [space.id, space] as const));
  return rows.map((r): MachineRow => {
    const dev = r.device ? devices.get(r.device) : undefined;
    const own = r.spaceId ? spaceById.get(r.spaceId) : undefined;
    const row: MachineRow = {
      id: r.id,
      name: r.name,
      via: r.via,
      online: r.online,
      os: r.os,
      limits: r.stoppedSharing ? withNotSharing([], stoppedSharingLine(r.name)) : [],
      merged: true,
    };
    // The core's words for the row, the same the SwiftUI list shows.
    if (r.status) row.status = r.status;
    if (r.spacesLabel != null) row.spacesLabel = r.spacesLabel;
    const detail = r.current ? machines.thisMachine?.detail : r.deviceOnly ? dev?.detail : own?.detail || dev?.detail;
    if (detail) row.detail = detail;
    if (r.lastSeen != null) row.lastSeen = r.lastSeen;
    if (r.current) {
      row.current = true;
      if (machines.host) row.host = toHostStatus(machines.host);
      if (machines.accessNotice) row.accessNotice = machines.accessNotice;
    }
    if (r.deviceOnly) {
      row.device = true;
      const state = machines.deviceStates?.[r.id];
      if (state) row.deviceState = state;
    }
    return row;
  });
}

const rowsById = (s: WkSettings) => new Map(s.page.sections.flatMap((sec) => sec.rows).map((r) => [r.id, r] as const));
const active = (r: WkSettingsRow | undefined) => r?.options.find((o) => o.active)?.id;
const lockedBy = (r: WkSettingsRow | undefined) => (r && !r.enabled && r.help?.startsWith("Set by ") ? r.help.slice(7) : undefined);

/** The core's Settings page back to the values `useSettings` reads. */
export function toSettingsSnapshot(s: WkSettings, page: Pick<SettingsValues, "theme" | "hotkey">): SettingsSnapshot {
  const rows = rowsById(s);
  const telemetry = rows.get("telemetry");
  const loginRow = rows.get("launch-at-login");
  const telemetryOn = telemetry ? active(telemetry) === "on" : DEFAULT_SETTINGS.telemetry;
  // The rows only this host lays out: what they show now, so the core lays
  // them out again on the page (null: the host has none).
  const on = (id: string) => (rows.has(id) ? active(rows.get(id)) === "on" : null);
  const protection = [...rows.values()]
    .filter((r) => r.id.startsWith("keyvault-protection:"))
    .map((r) => ({ label: r.label, value: r.value ?? "" }));
  return {
    values: {
      theme: page.theme,
      hotkey: page.hotkey,
      menuBar: active(rows.get("notch")) === "hide",
      telemetry: telemetryOn,
      defaultLocation: DEFAULT_SETTINGS.defaultLocation,
      // The core lays out `launch-at-login` once the host read the login item.
      launchAtLogin: loginRow && loginRow.enabled ? active(loginRow) === "on" : null,
      updateChannel: s.updateChannel === "beta" || s.updateChannel === "stable" ? s.updateChannel : null,
      // The core's General row (`settings::auto_connect_row`), when the host lays it out.
      ...(rows.has("auto-connect") ? { autoConnect: active(rows.get("auto-connect")) === "on" } : {}),
      // The core's own settings the page changes (`settings.update`); an older host has none.
      ...(s.tips ? { tips: { off: s.tips.off === true, done: Array.isArray(s.tips.done) ? s.tips.done : [] } } : {}),
      ...(Array.isArray(s.notchPinned) ? { notchPinned: s.notchPinned.filter((id): id is string => typeof id === "string") } : {}),
    },
    defaultLocation: null,
    telemetry: telemetry
      ? {
          enabled: telemetryOn,
          source: lockedBy(telemetry) ?? "config",
          sourceKind: lockedBy(telemetry) ? "env" : "config",
          noticeShown: true,
          noticeText: "",
          docsUrl: "",
        }
      : null,
    hostSettings: {
      runtimes: s.runtimes ?? null,
      autoConnect: on("auto-connect"),
      overflow: on("placement-overflow"),
      keyvaultAutoWipe: on("keyvault-auto-wipe"),
      keyvaultUnlockPrompt: on("keyvault-unlock-prompt"),
      keyvaultSiteIcons: on("keyvault-site-icons"),
      keyvaultProtection: protection.length ? protection : null,
    },
    globalShortcut: s.globalShortcut ? { label: s.globalShortcut.label, registered: s.globalShortcut.registered } : null,
    accountUrl: s.accountUrl ?? null,
    cli: s.cli && s.cli.name ? { name: s.cli.name, installed: Boolean(s.cli.installed) } : null,
  };
}

export function toSessionSnapshot(s: WkSession, onboarding: WkOnboarding | null = null): SessionSnapshot {
  return {
    account: {
      configured: s.cloudConfigured || s.signedIn,
      authMode: s.signedIn ? "user" : "none",
      identity: opt(s.identity),
    },
    // The SwiftUI app's first run is its own window; Electron's is the page's.
    onboarding: onboarding ? { completed: onboarding.completed, mode: onboarding.mode } : { completed: true, mode: "client" },
    daemon: null,
    // The host runs the sign-in: while it waits, its code and page; when
    // it failed, why (the page shows it instead of waiting on).
    hostSignIn:
      typeof s.signIn === "object" && s.signIn.type === "waiting"
        ? { userCode: opt(s.signIn.userCode) ?? null, url: opt(s.signInUrl) ?? null, opened: s.signIn.opened !== false }
        : null,
    hostSignInFailure:
      typeof s.signIn === "object" && s.signIn.type === "failed"
        ? { reason: s.signIn.reason ?? "unknown", message: s.signIn.message ?? "", detail: opt(s.signIn.detail) ?? null }
        : null,
  };
}

const snakeKey = (k: string) => k.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);

/** The broker's records are snake_case on the wire (`wire.rs`); Swift's are camelCase. */
function snake(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(snake);
  if (v && typeof v === "object") {
    return Object.fromEntries(Object.entries(v).filter(([k]) => k !== "blob").map(([k, x]) => [snakeKey(k), x === null ? undefined : snake(x)]));
  }
  return v;
}

const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/** A Swift enum (`"unsigned"`, or `{type: "adHoc", ...}`) back to the
 * broker's `kind`-tagged enum (`{kind: "ad_hoc", ...}`). */
function tagged(v: unknown): unknown {
  if (typeof v === "string") return { kind: snakeKey(v) };
  if (!v || typeof v !== "object") return v;
  const { type, ...rest } = v as Record<string, unknown>;
  return { kind: snakeKey(String(type)), ...(snake(rest) as object) };
}

/** A waiting request (`KvPending`): snake_case, with its selectors and the
 * caller's signing as the broker tags them. */
export function toPending(p: unknown): KvPending {
  const r = (p ?? {}) as Record<string, unknown>;
  const caller = (r.caller ?? {}) as Record<string, unknown>;
  const request = (r.request ?? {}) as Record<string, unknown>;
  const out = snake({ ...r, caller: { ...caller, signing: null }, request: { ...request, selectors: null }, needsImport: null }) as Record<string, unknown>;
  return {
    ...out,
    caller: { ...(out.caller as object), signing: tagged(caller.signing) },
    request: { ...(out.request as object), selectors: list(request.selectors).map(tagged) },
    needs_import: list(r.needsImport).map(tagged),
  } as unknown as KvPending;
}

/** `HostModel.state` back to `host_status`. */
export function toHostStatus(h: WkHostState): HostStatus {
  return {
    configured: h.configured,
    mode: h.mode === "relay" || h.mode === "direct" ? h.mode : null,
    relayUrl: h.relayUrl ?? null,
    directUrl: h.directUrl ?? null,
    machineId: h.machineId ?? null,
    name: h.name ?? null,
    sharing: h.sharing,
    service: { installed: h.serviceInstalled, running: h.serviceRunning, kind: h.serviceKind },
    online: h.online ?? null,
    clients: list(h.clients).map((c) => {
      const x = c as WkHostState["clients"][number];
      return { id: x.id, email: opt(x.email), name: opt(x.name), streams: opt(x.streams) };
    }),
    permissions: list(h.permissions).map((p) => {
      const x = p as WkHostState["permissions"][number];
      return { id: x.id, label: x.title, instructions: opt(x.instructions), settingsUrl: opt(x.settingsUrl), granted: x.granted };
    }),
    error: h.error ?? null,
    shareDesktop: h.shareDesktop,
    provideSpaces: h.provideSpaces,
    maxSpaces: h.maxSpaces,
    // The rest of the core's state, as the host sends it: the logs, the
    // provided Spaces, and whether sharing waits for a sign-in.
    maxMacosVms: h.maxMacosVms ?? 0,
    maxMemoryGb: h.maxMemoryGb ?? 0,
    maxDiskGb: h.maxDiskGb ?? 0,
    maxRunMinutes: h.maxRunMinutes ?? 0,
    usedMemoryGb: h.usedMemoryGb ?? 0,
    usedDiskGb: h.usedDiskGb ?? 0,
    totalMemoryGb: h.totalMemoryGb ?? 0,
    totalDiskGb: h.totalDiskGb ?? 0,
    recentAccess: list(h.recentAccess) as HostStatus["recentAccess"],
    accessLogError: h.accessLogError ?? null,
    providedSpaces: list(h.providedSpaces) as HostStatus["providedSpaces"],
    spacesAudit: list(h.spacesAudit) as HostStatus["spacesAudit"],
    spacesAuditError: h.spacesAuditError ?? null,
    pausedSignedOut: Boolean(h.pausedSignedOut),
    owner: h.owner ?? null,
    ownerEmail: h.ownerEmail ?? null,
    account: h.account ?? null,
    progress: h.progress ?? null,
    appName: h.appName ?? null,
  };
}

/** `KeyvaultModel.overview` back to the wire overview. */
export function toKeyvaultOverview(k: WkKeyvault): KeyvaultOverview {
  const o = k.overview ?? {};
  return {
    availability: k.availability,
    message: (o.message as string | null | undefined) ?? undefined,
    status: o.status ? (snake(o.status) as KeyvaultOverview["status"]) : undefined,
    serverVerified: Boolean(o.serverVerified),
    items: snake(list(o.items)) as KeyvaultOverview["items"],
    namesVisible: Boolean(o.namesVisible),
    itemsTotal: Number(o.itemsTotal ?? list(o.items).length),
    pending: list(o.pending).map(toPending),
    grants: snake(list(o.grants)) as KeyvaultOverview["grants"],
    rules: snake(list(o.rules)) as KeyvaultOverview["rules"],
    deliveries: snake(list(o.deliveries)) as KeyvaultOverview["deliveries"],
    audit: snake(list(o.audit)) as KeyvaultOverview["audit"],
    auditVerification: o.auditVerification ? (snake(o.auditVerification) as KeyvaultOverview["auditVerification"]) : undefined,
    partialErrors: list(o.partialErrors) as string[],
    dismissed: list(k.dismissed) as string[],
  };
}

const EVENTS: Record<string, HostEvent[]> = {
  "spaces.changed": [{ type: "spaces.changed" }, { type: "machines.changed" }],
  "machines.changed": [{ type: "machines.changed" }],
  "agents.changed": [{ type: "agents.changed" }],
  "checklist.changed": [{ type: "checklist.changed" }],
  "keyvault.changed": [{ type: "keyvault.changed" }],
  "settings.changed": [{ type: "settings.changed" }],
  "update.changed": [{ type: "update.changed" }],
  "session.changed": [{ type: "session.changed" }],
};

/** The window background for an appearance (the page's own `--background`). */
const BACKGROUND = { light: "#f7f8fa", dark: "#16181c" } as const;

/* ---- The adapter ---------------------------------------------------------------- */

export function createWebkitAdapter(
  win: HostWindow = globalThis.window as HostWindow,
  options: WebkitAdapterOptions = {},
): DataAdapter {
  const electron = options.host === "electron";
  const transport: HostTransport | null = electron ? electronTransport(win) : webkitTransport(win);
  if (!transport) throw new Error(electron ? "window.cuaDesktop is not available" : "window.webkit.messageHandlers.cua is not available");
  const os = hostOs(win);
  const timeoutMs = options.timeoutMs ?? 30_000;
  const events = new Emitter();
  const pending = new Set<(e: Error) => void>();
  let seq = 0;
  const prefix = Math.random().toString(36).slice(2, 8);
  // Page-side settings (the SwiftUI host has no appearance or hotkey setting).
  const page: Pick<SettingsValues, "theme" | "hotkey"> = { theme: DEFAULT_SETTINGS.theme, hotkey: DEFAULT_SETTINGS.hotkey };

  /** `wait`: how long to wait for the answer (ms), null for as long as it
   * takes (a create answers when the Space is ready). */
  function request<T>(method: HostMethod, args: Record<string, unknown> = {}, wait: number | null = timeoutMs): Promise<T> {
    const message: WebkitRequest = { id: `${prefix}-${++seq}`, method, args };
    return new Promise<T>((resolve, reject) => {
      const fail = (e: Error) => {
        clearTimeout(timer);
        pending.delete(fail);
        reject(e);
      };
      const timer = wait === null ? undefined : setTimeout(() => fail(new HostError(`${method}: the app did not answer in ${wait} ms`, "timeout")), wait);
      pending.add(fail);
      let reply: unknown;
      try {
        reply = transport!.post(message);
      } catch (e) {
        fail(e instanceof Error ? e : new Error(String(e)));
        return;
      }
      Promise.resolve(reply).then(
        (r) => {
          if (!pending.has(fail)) return;
          clearTimeout(timer);
          pending.delete(fail);
          if (!isWebkitResponse(r)) reject(new HostError(`${method}: the app answered without an envelope`, "protocol"));
          else if (r.ok) resolve(r.result as T);
          else {
            const e = r.error;
            reject(new HostError(e?.message ?? `${method} failed`, e?.code, e && { title: e.title, details: e.details, actionLabel: e.actionLabel }));
          }
        },
        (e: unknown) => {
          if (pending.has(fail)) fail(e instanceof Error ? e : new Error(String(e)));
        },
      );
    });
  }

  const settingsSnapshot = async () => toSettingsSnapshot(await request<WkSettings>("settings.get"), page);
  const choose = (row: string, option: string) => request<WkSettings>("settings.choose", { row, option });

  function setTheme(theme: SettingsValues["theme"]): void {
    page.theme = theme;
    const dark = theme === "dark" || (theme === "system" && Boolean(win.matchMedia?.("(prefers-color-scheme: dark)").matches));
    // `appearance`: the Electron shell follows it for its own chrome (the
    // title bar, its controls); the SwiftUI host reads only `color`.
    void request("window.setBackgroundColor", { color: BACKGROUND[dark ? "dark" : "light"], appearance: theme }).catch(() => {});
  }


  // Creates this page started: the host lists each as its own pending row
  // too, which the store already draws.
  const ownCreates = new Set<string>();

  // What the host said about its last registry read (`listNotice`).
  let rosterError: string | null = null;
  // The host's standing notice (`hostNotice`).
  let daemonNotice: string | null = null;
  // How far the host's last list read got (`listStatus`).
  let listStatus: ListStatus = { loaded: false, incomplete: false, reaching: 0, stale: false };
  // What your other computers are creating (`creatingElsewhere`).
  let creatingElsewhere: CreatingElsewhere[] = [];
  // The Machines page the host's core drew (`machinesView`).
  let machinesView: MachinesView | null = null;
  const listSpaces = async () => {
    const answer = await request<WkSpaces>("spaces.list");
    rosterError = answer.rosterError ?? null;
    daemonNotice = answer.daemonNotice ?? null;
    // Until the host's first read answers its list is no list at all (an
    // app just updated, its daemon starting): never "no Spaces".
    listStatus = { loaded: answer.loaded !== false, incomplete: answer.incomplete === true, reaching: answer.reaching ?? 0, stale: answer.stale === true };
    creatingElsewhere = answer.creatingElsewhere ?? [];
    return answer;
  };

  const ops: { [K in OpName]: (args: OpArgs<K>) => Promise<OpResult<K>> } = {
    "spaces.list": async () => (await listSpaces()).spaces.filter((s) => !ownCreates.has(s.space.id)).map((s) => spaceToRow(s.space)),
    "spaces.create": async ({ config, pendingId, os, notifyWhenReady }) => {
      ownCreates.add(pendingId);
      const args = notifyWhenReady ? { config, pendingId, os: os ?? null, notifyWhenReady } : { config, pendingId, os: os ?? null };
      return spaceToRow(await request<WkSpace>("spaces.create", args, null));
    },
    "spaces.cancelCreate": ({ pendingId }) => request<CancelOutcome>("spaces.cancelCreate", { pendingId }),
    "spaces.setPower": async ({ spaceId, on }): Promise<SpacePowerReport> => {
      await request("spaces.setPower", { id: spaceId, on });
      return { space: spaceId, state: on ? "running" : "suspended", power: on ? "on" : "off", message: "" };
    },
    "spaces.delete": async ({ spaceId, removeOnly }) => {
      // `AppModel.delete(_:removeOnly:)`: only "Remove from List" sends it.
      await request("spaces.delete", removeOnly ? { id: spaceId, removeOnly: true } : { id: spaceId });
      return "";
    },
    // The host marks each row deleting as it goes and answers once all are done.
    "spaces.deleteMany": ({ ids }) => request<{ results: BulkResult[] }>("spaces.deleteMany", { ids }, null),
    "spaces.leave": ({ ids }) => request<{ results: BulkResult[] }>("spaces.leave", { ids }, null),
    // `{}`; the new name arrives with the next `spaces.list`.
    "spaces.rename": async ({ spaceId, name }) => {
      await request("spaces.rename", { spaceId, name });
      return null;
    },
    "spaces.open": async ({ spaceId }) => {
      await request("spaces.open", { id: spaceId });
      return null;
    },
    "machines.list": async () => {
      const [spaces, machines] = await Promise.all([listSpaces(), request<WkMachines>("machines.list").catch(() => null)]);
      machinesView = machines?.view ? fromHostView(machines.view) : null;
      return toMachineRows(spaces, machines, os);
    },
    "host.status": async () => {
      const h = await request<WkHostState | null>("host.status");
      if (!h) throw new HostError("This Mac has not reported its sharing status yet", "not_found");
      return toHostStatus(h);
    },
    "settings.get": settingsSnapshot,
    "settings.choose": async ({ row, option }) => {
      await choose(row, option);
      return settingsSnapshot();
    },
    "settings.set": async ({ key, value }) => {
      switch (key) {
        case "theme":
          setTheme(value as SettingsValues["theme"]);
          break;
        case "hotkey":
          page.hotkey = String(value);
          break;
        case "menuBar":
          await choose("notch", value ? "hide" : "show");
          break;
        case "telemetry":
          await choose("telemetry", value ? "on" : "off");
          break;
        case "defaultLocation":
          if (value !== "local") throw new HostError("New Spaces can start on This Mac here", "unsupported");
          break;
        case "launchAtLogin":
          await choose("launch-at-login", value ? "on" : "off");
          break;
        case "updateChannel":
          await choose("update-channel", String(value));
          break;
        // The app core's own settings (`AppSettings`), saved as given.
        case "tips":
        case "notchPinned":
          return toSettingsSnapshot(await request<WkSettings>("settings.update", { [key]: value ?? null }), page);
      }
      return settingsSnapshot();
    },
    "keyvault.overview": async () => toKeyvaultOverview(await request<WkKeyvault>("keyvault.get")),
    "keyvault.unlock": async ({ passphrase }) => {
      if (passphrase) throw new HostError("Cua Spaces asks for the passphrase in its own prompt", "native_only");
      await request("keyvault.unlockVault");
      return null;
    },
    "keyvault.setUnattended": async ({ itemIds, unattended }) => {
      const k = await request<WkKeyvault>(unattended ? "keyvault.unlock" : "keyvault.lock", { ids: itemIds });
      const wanted = new Set(itemIds);
      return toKeyvaultOverview(k).items.filter((i) => wanted.has(i.id));
    },
    "keyvault.setDisabled": async ({ disabled }) => {
      await request("keyvault.setDisabled", { disabled });
      return null;
    },
    "keyvault.approve": async ({ requestId, items }) => snake(await request("keyvault.approve", { requestId, items: items ?? null })) as KvGrant,
    "keyvault.deny": async ({ requestId }) => {
      await request("keyvault.deny", { requestId });
      return null;
    },
    "keyvault.revokeGrant": async ({ id }) => Number(await request<number>("keyvault.revokeGrant", { id })),
    "session.get": async () => {
      // A SwiftUI host from before the shared first run has no `onboarding.get`: it ran its own.
      const [s, onboarding] = await Promise.all([request<WkSession>("session.get"), request<WkOnboarding>("onboarding.get").catch(() => null)]);
      return toSessionSnapshot(s, onboarding);
    },
    "session.signIn": async (args) => {
      const s = await request<WkSession>("session.signIn", args?.code ? { code: true } : {});
      const userCode = typeof s.signIn === "object" ? opt(s.signIn.userCode) : undefined;
      return { method: userCode ? "device" : "browser", userCode, verificationUri: opt(s.signInUrl) ?? "" };
    },
    "session.signOut": async () => {
      await request("session.signOut");
      return null;
    },
    "session.completeOnboarding": async ({ mode, launchAtLogin }) => {
      await request("onboarding.complete", launchAtLogin === undefined ? { mode } : { mode, launchAtLogin });
      return null;
    },

    "agents.list": async () => list(await request("agents.list")) as PersistentAgent[],
    "agents.runs": async ({ spaceId }) => list(await request("agents.runs", { spaceId })) as SpaceAgentRun[],
    "agents.events": ({ spaceId, runId, cursor, max }) =>
      request<AgentEventsPage>("agents.events", max == null ? { spaceId, runId, cursor } : { spaceId, runId, cursor, max }),
    "agents.start": (a) => request<AgentStarted>("agents.start", { ...a }),
    "agents.message": (a) => request<AgentMessageSent>("agents.message", { ...a }),
    "agents.pause": async ({ name }) => {
      await request("agents.pause", { name });
      return null;
    },
    "agents.resume": async ({ name }) => {
      await request("agents.resume", { name });
      return null;
    },
    "agents.setup": async () => list(await request("agents.setup")) as AgentSetupRow[],
    "agents.configure": async ({ agents }) => list(await request("agents.configure", { agents: agents ?? null })) as AgentSetupRow[],
    "agents.signIn": async ({ agent }) => list(await request("agents.signIn", { agent })) as AgentSetupRow[],
    "agents.status": ({ refresh }) => request<ConnectionReport>("agents.status", { refresh: refresh ?? false }),
    "agents.connect": async ({ agents, signIn, skills, mcp }) =>
      // No timeout: Claude Code's sign-in waits for its one browser approval.
      list(await request("agents.connect", { agents, signIn, skills: skills ?? true, mcp: mcp ?? true }, null)) as ConnectResult[],
    "agents.disconnect": ({ agents, signIn, skills, mcp }) =>
      request<{ removed_sign_ins: string[] }>("agents.disconnect", { agents, signIn, ...(skills === undefined ? {} : { skills }), ...(mcp === undefined ? {} : { mcp }) }),
    "agents.signInStart": ({ agent }) => request<OpResult<"agents.signInStart">>("agents.signInStart", { agent }),
    "agents.signInWait": ({ id }) => request<OpResult<"agents.signInWait">>("agents.signInWait", { id }),
    "session.openExternal": async ({ url }) => {
      win.open?.(url, "_blank", "noopener");
      return null;
    },
    ...webkitSettingsOps(request),
    ...webkitPageOps(request, toHostStatus, spaceToRow),
    ...webkitStartupOps(request),
    ...webkitTelemetryNoticeOps(request),
    ...webkitAgentDetailOps(request),
    ...webkitThreadOps(request),
    ...webkitAgentKeysOps(request),
    ...webkitKeyvaultManageOps(request, toKeyvaultOverview as (wk: never) => KeyvaultOverview),
    ...webkitKeyvaultSetupOps(request, () => new HostError("Cua Spaces asks for the passphrase in its own prompt", "native_only")),
    // Electron draws a Space's video in the page, on a ticket from the
    // host; the Mac app draws it natively.
    ...(electron
      ? { "spaces.openStream": (target: OpArgs<"spaces.openStream">) => request<StreamTicket>("spaces.openStream", { ...target }) }
      : unsupportedStreamOps("webkit")),
    "relay.usage": () => request<RelayUsage | null>("relay.usage"),
    "nudges.pending": () => request<RiskyRequest[]>("nudges.pending"),
    "nudges.answer": ({ id, choice }) => request<null>("nudges.answer", { id, choice }),
    "resources.get": ({ visible }) => request<ResourcesSnapshot | null>("resources.get", { visible }),
    "machines.rename": async ({ id, name }) => {
      await request("machines.rename", { id, name });
      return null;
    },
    "machines.checkHealth": async () => {
      await request("machines.checkHealth");
      return null;
    },
    "machines.remove": async ({ machine, device }) => {
      await request("machines.remove", { machine: machine ?? null, device: device ?? null });
      return null;
    },
    "checklist.get": () => request<{ state: HostChecklistState }>("checklist.get"),
    "checklist.send": ({ action }) => request<{ state: HostChecklistState }>("checklist.send", { action }),
    "links.list": () => request<{ links: ShareLink[] }>("links.list"),
    "links.create": (a: LinkCreate) => request<ShareLink>("links.create", a as unknown as Record<string, unknown>),
    "links.stop": ({ id }) => request<{ stopped: string[] }>("links.stop", { id }),
    // The bundled CLI on PATH (the app core's installer in the host).
    "cli.plan": () => request<CliInstallPlan | null>("cli.plan"),
    "cli.install": ({ modifyPath }) => request<CliInstallPlan>("cli.install", { modifyPath }),
    // Presence over the page's own video: the Mac app draws it natively.
    ...(electron
      ? {
          "presence.join": (args: OpArgs<"presence.join">) => request<PresenceJoinInfo>("presence.join", { ...args }),
          "presence.publish": (args: OpArgs<"presence.publish">) => request<null>("presence.publish", { ...args }),
          "presence.leave": (args: OpArgs<"presence.leave">) => request<null>("presence.leave", { ...args }),
        }
      : unsupportedPresenceOps("webkit")),
    // Approving an agent's cloud: the daemon's person-only `cloud_approve`.
    ...cloudApproveWebkitOps(request),
    // Windows' "Install Docker Desktop": a Mac sets up its runtimes itself.
    ...(electron
      ? { "spaces.fixRuntime": (args: OpArgs<"spaces.fixRuntime">) => request<null>("spaces.fixRuntime", { ...args }) }
      : unsupportedRuntimeFixOps("webkit")),
  };

  const onEvent = (detail: WebkitEventDetail) => {
    if (detail?.event === "spaces.createProgress") {
      const p = detail.payload as CreateProgress | null;
      if (p && typeof p.pendingId === "string") events.emit({ type: "spaces.createProgress", progress: p });
      return;
    }
    if (detail?.event === "spaces.newRequested") {
      const on = (detail.payload as { on?: unknown } | null)?.on;
      requestNewSpace(typeof on === "string" && on ? on : null);
      return;
    }
    // The Electron menu's Settings… (⌘,) and Help > Getting Started: the same window event the SwiftUI
    // host sends, which the page's key bindings answer (the SwiftUI host's
    // already arrives as one).
    if (detail?.event === WEBKIT_OPEN_SETTINGS_EVENT || detail?.event === WEBKIT_OPEN_CHECKLIST_EVENT) {
      if (electron) (win as unknown as Partial<Window>).dispatchEvent?.(new CustomEvent(WEBKIT_EVENT, { detail }));
      return;
    }
    if (detail?.event === "startup.changed") {
      events.emit({ type: "startup.changed", state: startupFromHost(detail.payload) });
      return;
    }
    // A real window dragged over a Space tile (the host follows the drag).
    if (detail?.event === WINDOW_DROP_EVENT) {
      setWindowDropTarget(windowDropTargetOf(detail.payload));
      return;
    }
    if (detail?.event === "teleport.progress") {
      const ev = teleportProgressFromWk(detail.payload);
      if (ev) events.emit(ev);
      return;
    }
    for (const ev of EVENTS[detail?.event ?? ""] ?? []) events.emit(ev);
  };
  const stopEvents = transport.listen(onEvent);

  // In the page only (not in the tests' stand-in windows). Electron reads
  // the drag regions from the CSS (`app-region`) itself.
  const dom = win as unknown as Partial<Window>;
  const stopDragRegions =
    !electron && dom.document && dom.requestAnimationFrame && typeof MutationObserver !== "undefined"
      ? syncDragRegions(dom as Window, (rects) => void request("window.setDragRegions", { rects }).catch(() => {}))
      : null;
  // Where the Space tiles are, for a real window dragged onto one (both
  // hosts: neither sees such a drag in the page; `../window-drop.ts`).
  const stopDropTargets =
    dom.document && dom.requestAnimationFrame && typeof MutationObserver !== "undefined"
      ? syncDropTargets(dom as Window, (targets) => void request("window.setDropTargets", { targets }).catch(() => {}))
      : null;

  return {
    mode: electron ? "electron" : "webkit",
    call<K extends OpName>(op: K, args: OpArgs<K>): Promise<OpResult<K>> {
      return (ops[op] as (a: OpArgs<K>) => Promise<OpResult<K>>)(args ?? ({} as OpArgs<K>));
    },
    subscribe: (l) => events.subscribe(l),
    listNotice: () => rosterError,
    listStatus: () => listStatus,
    creatingElsewhere: () => creatingElsewhere,
    hostNotice: () => daemonNotice,
    machinesView: () => machinesView,
    dispose() {
      for (const fail of [...pending]) fail(new HostError("the bridge was closed", "closed"));
      events.clear();
      stopEvents();
      stopDragRegions?.();
      stopDropTargets?.();
      setWindowDropTarget(null);
    },
  };
}

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
 * The SwiftUI app's first run is native, so there `session.completeOnboarding`
 * resolves without a call; the Electron shell's is this page's
 * (`onboarding.get`, `onboarding.complete`). Appearance and the hotkey are page-side; an appearance change sets
 * the window's background (`window.setBackgroundColor`). The page tells the
 * window where it drags (`window.setDragRegions`, `../drag-regions.ts`).
 */

import { Emitter, HostError, type DataAdapter } from "../adapter";
import { hostOs, THIS_MACHINE, type HostOs } from "../host-os";
import { electronTransport, webkitTransport, type HostTransport } from "../transport";
import type { AgentEventsPage, AgentSetupRow, PersistentAgent, SpaceAgentRun } from "../contracts/agents";
import type { CancelOutcome, DefaultLocation, HostStatus, MachineRow, SpacePowerReport } from "../contracts/host";
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
import { requestNewSpace } from "../new-space";
import { STOPPED_SHARING_ERROR, stoppedSharingLine, withNotSharing } from "../sharing";
import { teleportProgressFromWk, webkitPageOps } from "../ops/webkit-pages";
import { webkitAgentKeysOps } from "../ops/agent-keys";
import { webkitKeyvaultManageOps } from "../ops/keyvault-manage";
import { webkitKeyvaultSetupOps } from "../ops/keyvault-setup";
import { webkitSettingsOps } from "../ops/settings";
import { startupFromHost, webkitStartupOps } from "../ops/startup";
import { unsupportedStreamOps, type StreamTicket } from "../ops/stream";
import type { SpaceRow } from "../contracts/spaces";
import {
  isWebkitResponse,
  WEBKIT_EVENT,
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
    provider: s.provider ?? (here ? "local" : "cloud"),
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
    cloud: opt(s.cloud),
    cloudPlace: opt(s.cloudPlace),
    cloudDelete: opt(s.cloudDelete),
    hostProgress: s.id.startsWith("pending:") ? (s.progress ?? undefined) : undefined,
  };
}

/** This Mac, the machines that provide the listed Spaces (and those that
 * share their own desktop, `relay:<machine>`), then the account's other
 * devices, marked `device`. Each relay machine carries what the relay
 * says of it (`presence`) and the hostname its cua-spacesd reported, so
 * the store lists a machine and the device on it once
 * (`mergeMachineRows`). */
export function toMachineRows(spaces: WkSpaces, machines: WkMachines | null, os: HostOs = "macos"): MachineRow[] {
  const self: MachineRow = { id: "this-mac", name: THIS_MACHINE[os], via: "local", online: true, os, current: true, limits: [] };
  if (machines?.thisMachine?.detail) self.detail = machines.thisMachine.detail;
  if (machines?.host) self.host = toHostStatus(machines.host);
  if (machines?.accessNotice) self.accessNotice = machines.accessNotice;
  const rows: MachineRow[] = [self];
  const byId = new Map(rows.map((r) => [r.id, r] as const));
  const add = (row: MachineRow) => {
    byId.set(row.id, row);
    rows.push(row);
  };
  for (const { space } of spaces.spaces) {
    // A machine's own desktop: `relay:<machine>` with no host above it.
    const own = space.provider === "relay" && !space.host && space.id.startsWith("relay:") ? space.id.slice(6) : undefined;
    if (own && !own.includes("/")) {
      // The list's connect probe was refused by a machine that stopped
      // sharing: it is connected (that is how it refused), not offline.
      const stopped = STOPPED_SHARING_ERROR.test(space.sdk?.error ?? "");
      const online = space.status === "running" || space.status === "local" || stopped;
      const limits = stopped ? withNotSharing([], stoppedSharingLine(space.name)) : [];
      const known = byId.get(own);
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

const rowsById = (s: WkSettings) => new Map(s.page.sections.flatMap((sec) => sec.rows).map((r) => [r.id, r] as const));
const active = (r: WkSettingsRow | undefined) => r?.options.find((o) => o.active)?.id;
const lockedBy = (r: WkSettingsRow | undefined) => (r && !r.enabled && r.help?.startsWith("Set by ") ? r.help.slice(7) : undefined);

/** The core's Settings page back to the values `useSettings` reads. */
export function toSettingsSnapshot(s: WkSettings, page: Pick<SettingsValues, "theme" | "hotkey">): SettingsSnapshot {
  const rows = rowsById(s);
  const location = rows.get("default-location");
  const telemetry = rows.get("telemetry");
  const loginRow = rows.get("launch-at-login");
  const env = lockedBy(location);
  const defaultLocation: DefaultLocation | null = location
    ? { value: active(location) ?? "local", source: env ? "env" : "config", env, path: "" }
    : null;
  const telemetryOn = telemetry ? active(telemetry) === "on" : DEFAULT_SETTINGS.telemetry;
  // The rows only this host lays out: what they show now, so the core lays
  // them out again on the page (null: the host has none).
  const source = (id: string) => (rows.has(id) ? (active(rows.get(id)) ?? "auto") : null);
  const on = (id: string) => (rows.has(id) ? active(rows.get(id)) === "on" : null);
  return {
    values: {
      theme: page.theme,
      hotkey: page.hotkey,
      menuBar: active(rows.get("notch")) === "hide",
      telemetry: telemetryOn,
      defaultLocation: defaultLocation?.value ?? DEFAULT_SETTINGS.defaultLocation,
      // The core lays out `launch-at-login` once the host read the login item.
      launchAtLogin: loginRow && loginRow.enabled ? active(loginRow) === "on" : null,
      updateChannel: s.updateChannel === "beta" || s.updateChannel === "stable" ? s.updateChannel : null,
      // The core's General row (`settings::auto_connect_row`), when the host lays it out.
      ...(rows.has("auto-connect") ? { autoConnect: active(rows.get("auto-connect")) === "on" } : {}),
    },
    defaultLocation,
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
      lumeSource: source("macos-runtime"),
      linuxSource: source("linux-runtime"),
      autoConnect: on("auto-connect"),
      keyvaultAutoWipe: on("keyvault-auto-wipe"),
    },
  };
}

export function toSessionSnapshot(s: WkSession, onboarding: WkOnboarding | null = null): SessionSnapshot {
  return {
    fleet: {
      configured: s.cloudConfigured || s.signedIn,
      authMode: s.signedIn ? "user" : "none",
      baseUrl: "",
      tokenUrl: "",
      identity: opt(s.identity),
    },
    // The SwiftUI app's first run is its own window; Electron's is the page's.
    onboarding: onboarding ? { completed: onboarding.completed, mode: onboarding.mode } : { completed: true, mode: "client" },
    daemon: null,
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
  "keyvault.changed": [{ type: "keyvault.changed" }],
  "settings.changed": [{ type: "settings.changed" }],
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
  const listSpaces = async () => {
    const answer = await request<WkSpaces>("spaces.list");
    rosterError = answer.rosterError ?? null;
    return answer;
  };

  const ops: { [K in OpName]: (args: OpArgs<K>) => Promise<OpResult<K>> } = {
    "spaces.list": async () => (await listSpaces()).spaces.filter((s) => !ownCreates.has(s.space.id)).map((s) => spaceToRow(s.space)),
    "spaces.create": async ({ config, pendingId, os }) => {
      ownCreates.add(pendingId);
      return spaceToRow(await request<WkSpace>("spaces.create", { config, pendingId, os: os ?? null }, null));
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
    "spaces.open": async ({ spaceId }) => {
      await request("spaces.open", { id: spaceId });
      return null;
    },
    "machines.list": async () => {
      const [spaces, machines] = await Promise.all([listSpaces(), request<WkMachines>("machines.list").catch(() => null)]);
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
          if (value !== "local" && value !== "cloud") throw new HostError("New Spaces can start on This Mac or Cua Cloud here", "unsupported");
          await choose("default-location", value);
          break;
        case "launchAtLogin":
          await choose("launch-at-login", value ? "on" : "off");
          break;
        case "updateChannel":
          await choose("update-channel", String(value));
          break;
      }
      return settingsSnapshot();
    },
    "keyvault.overview": async () => toKeyvaultOverview(await request<WkKeyvault>("keyvault.get")),
    "keyvault.unlock": async ({ passphrase }) => {
      if (passphrase) throw new HostError("Enter the passphrase in the Cua Spaces window", "native_only");
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
      if (!electron) return toSessionSnapshot(await request<WkSession>("session.get"));
      const [s, onboarding] = await Promise.all([request<WkSession>("session.get"), request<WkOnboarding>("onboarding.get")]);
      return toSessionSnapshot(s, onboarding);
    },
    "session.signIn": async () => {
      const s = await request<WkSession>("session.signIn");
      const userCode = typeof s.signIn === "object" ? opt(s.signIn.userCode) : undefined;
      return { method: userCode ? "device" : "browser", userCode, verificationUri: "" };
    },
    "session.signOut": async () => {
      await request("session.signOut");
      return null;
    },
    "session.completeOnboarding": async ({ mode, launchAtLogin }) => {
      if (electron) await request("onboarding.complete", launchAtLogin === undefined ? { mode } : { mode, launchAtLogin });
      return null;
    },

    "agents.list": async () => list(await request("agents.list")) as PersistentAgent[],
    "agents.runs": async ({ spaceId }) => list(await request("agents.runs", { spaceId })) as SpaceAgentRun[],
    "agents.events": ({ spaceId, runId, cursor, max }) =>
      request<AgentEventsPage>("agents.events", max == null ? { spaceId, runId, cursor } : { spaceId, runId, cursor, max }),
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
    "session.openExternal": async ({ url }) => {
      win.open?.(url, "_blank", "noopener");
      return null;
    },
    ...webkitSettingsOps(request),
    ...webkitPageOps(request, toHostStatus, spaceToRow),
    ...webkitStartupOps(request),
    ...webkitAgentKeysOps(request),
    ...webkitKeyvaultManageOps(request, toKeyvaultOverview as (wk: never) => KeyvaultOverview),
    ...webkitKeyvaultSetupOps(request, () => new HostError("Enter the passphrase in the Cua Spaces window", "native_only")),
    // Electron draws a Space's video in the page, on a ticket from the
    // host; the Mac app draws it natively.
    ...(electron
      ? { "spaces.openStream": (target: OpArgs<"spaces.openStream">) => request<StreamTicket>("spaces.openStream", { ...target }) }
      : unsupportedStreamOps("webkit")),
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
    // The Electron menu's Settings… (⌘,): the same window event the SwiftUI
    // host sends, which the page's key bindings answer (the SwiftUI host's
    // already arrives as one).
    if (detail?.event === WEBKIT_OPEN_SETTINGS_EVENT) {
      if (electron) (win as unknown as Partial<Window>).dispatchEvent?.(new CustomEvent(WEBKIT_EVENT, { detail }));
      return;
    }
    if (detail?.event === "startup.changed") {
      events.emit({ type: "startup.changed", state: startupFromHost(detail.payload) });
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

  return {
    mode: electron ? "electron" : "webkit",
    call<K extends OpName>(op: K, args: OpArgs<K>): Promise<OpResult<K>> {
      return (ops[op] as (a: OpArgs<K>) => Promise<OpResult<K>>)(args ?? ({} as OpArgs<K>));
    },
    subscribe: (l) => events.subscribe(l),
    listNotice: () => rosterError,
    dispose() {
      for (const fail of [...pending]) fail(new HostError("the bridge was closed", "closed"));
      events.clear();
      stopEvents();
      stopDragRegions?.();
    },
  };
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The SwiftUI host's answers (`WebUIBridge+Pages.swift`) for the pages
 * beyond Spaces, the Keyvault and Agents: New Space's address form and
 * "Connect a cloud", Teleport and Share, Cua Volume, Settings, Notifications,
 * usage events, a Space's detail and This machine.
 *
 * Each method keeps its operation's name and arguments. The host answers the daemon tools' JSON as
 * is (the Volume, Storage and cloud tools, already the wire shapes) or the
 * app core's records as Swift encodes them (camelCase, enums as their case
 * name), which this maps onto the contracts: Swift's camelCase enum cases
 * (`installOnly`, `appWithFiles`, `signIns`) become the core's snake_case,
 * and the Keyvault's inventory becomes snake_case.
 *
 * `spaces.createOptions` answers the env the native New Space sheet opens
 * with (`AppModel.newSpaceEnv`), so the page's wizard runs on what the
 * SwiftUI app knows (`NewSpaceOptions.env`).
 */

import { HostError } from "../adapter";
import type { HostStatus } from "../contracts/host";
import type { SpaceRow } from "../contracts/spaces";
import type { CatalogEntry, KvInventory, RunEvent, TeleportPlan } from "../contracts/teleport";
import type { HostEvent, OpArgs, OpName, OpResult } from "../protocol";
import type { WebkitMethod, WkHostState, WkSpace } from "../webkit-protocol";
import { isSettingsPane } from "./host-setup";
import { sanitizeSignals } from "./telemetry";

/** `wait`: how long to wait for the answer (ms); null for as long as it takes. */
type Request = <T>(method: WebkitMethod, args?: Record<string, unknown>, wait?: number | null) => Promise<T>;
type Json = Record<string, unknown>;

/** The operations answered here (the rest of `OPERATIONS` are in `adapters/webkit.ts`). */
export const WEBKIT_PAGE_OPS = [
  "spaces.createOptions",
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
] as const satisfies readonly OpName[];

export type WebkitPageOp = (typeof WEBKIT_PAGE_OPS)[number];
type Handlers = { [K in WebkitPageOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };

/* ---- Swift's records onto the contracts --------------------------------------- */

const snakeWord = (v: unknown) => (typeof v === "string" ? v.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`) : v);
const words = (v: unknown) => (Array.isArray(v) ? v.map(snakeWord) : []);
const nul = <T>(v: T | undefined): T | null => (v === undefined ? null : v);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/** Keys and nested keys to snake_case (the Keyvault's wire records). */
function snake(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(snake);
  if (v && typeof v === "object") {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [String(snakeWord(k)), x === null ? undefined : snake(x)]));
  }
  return v;
}

/** `AppCatalogEntry` as the core's entry. The host keeps the SDK's entry by
 * id between the catalog and the plan, so `json` is the id. */
export function entryFromWk(j: Json): CatalogEntry {
  return {
    id: String(j.id),
    name: String(j.name ?? ""),
    hostPath: nul(j.hostPath as string | undefined),
    hostAppId: nul(j.hostAppId as string | undefined),
    version: nul(j.version as string | undefined),
    capability: snakeWord(j.capability) as CatalogEntry["capability"],
    reason: nul(j.reason as string | undefined),
    moves: words(j.moves) as CatalogEntry["moves"],
    providerId: nul(j.providerId as string | undefined),
    sensitiveGroups: words(j.sensitiveGroups) as CatalogEntry["sensitiveGroups"],
    installSource: nul(j.installSource as string | undefined),
    installId: nul(j.installId as string | undefined),
    installVersion: nul(j.installVersion as string | undefined),
    launchBin: nul(j.launchBin as string | undefined),
    lastUsedMs: typeof j.lastUsedMs === "number" ? j.lastUsedMs : null,
    json: String(j.id),
  };
}

/** `AppTeleportPlan` (its `json` is the key the host keeps the SDK's plan under). */
export function planFromWk(j: Json): TeleportPlan {
  return {
    app: entryFromWk((j.app ?? {}) as Json),
    spaceId: String(j.spaceId ?? ""),
    moves: snakeWord(j.moves) as TeleportPlan["moves"],
    steps: arr(j.steps).map((s) => ({ kind: String((s as Json).kind ?? ""), summary: String((s as Json).summary ?? "") })),
    consent: arr(j.consent).map((x) => {
      const c = x as Json;
      return {
        kind: snakeWord(c.kind) as TeleportPlan["consent"][number]["kind"],
        key: String(c.key ?? ""),
        label: String(c.label ?? ""),
        detail: String(c.detail ?? ""),
        bytes: Number(c.bytes ?? 0),
        sensitive: Boolean(c.sensitive),
      };
    }),
    sensitive: Boolean(j.sensitive),
    relayUnsealed: Boolean(j.relayUnsealed),
    totalBytes: Number(j.totalBytes ?? 0),
    warnings: arr(j.warnings).map(String),
    json: String(j.json ?? ""),
  };
}

export function runEventFromWk(j: Json): RunEvent {
  return {
    step: Number(j.step ?? 0),
    steps: Number(j.steps ?? 0),
    kind: String(j.kind ?? ""),
    phase: snakeWord(j.phase) as RunEvent["phase"],
    detail: String(j.detail ?? ""),
    doneBytes: Number(j.doneBytes ?? 0),
    totalBytes: Number(j.totalBytes ?? 0),
  };
}

/** `teleport.progress` from the host's event payload `{runId, event}`. */
export function teleportProgressFromWk(payload: unknown): HostEvent | null {
  const p = payload as { runId?: unknown; event?: unknown } | null;
  if (!p || typeof p.runId !== "string" || !p.event || typeof p.event !== "object") return null;
  return { type: "teleport.progress", runId: p.runId, event: runEventFromWk(p.event as Json) };
}

/* ---- The handlers ----------------------------------------------------------------- */

export function webkitPageOps(request: Request, toHostStatus: (h: WkHostState) => HostStatus, spaceToRow: (s: WkSpace) => SpaceRow): Handlers {
  type Op = WebkitPageOp;
  const call = (op: Op) => (args: object) => request<never>(op, { ...args });
  const none = (op: Op) => async (args: object) => {
    await request(op, { ...args });
    return null;
  };
  const list = (op: Op) => async (args: object) => arr(await request(op, { ...args })) as never;
  // Setup and Sign In wait while the person signs in in the browser: no
  // timeout, as for a create.
  const host = (op: Op) => async (args: object) => toHostStatus(await request<WkHostState>(op, { ...args }, null));

  return {
    "spaces.createOptions": call("spaces.createOptions"),
    "spaces.add": async (args) => spaceToRow(await request<WkSpace>("spaces.add", { ...args })),
    "clouds.status": async () => (await request<OpResult<"clouds.status"> | null>("clouds.status")) ?? { providers: [] },
    "clouds.test": call("clouds.test"),
    "clouds.connect": call("clouds.connect"),

    "teleport.catalog": async ({ spaceId }) => arr(await request("teleport.catalog", { spaceId })).map((e) => entryFromWk(e as Json)),
    "teleport.entryForPath": async ({ path }) => entryFromWk(await request<Json>("teleport.entryForPath", { path })),
    "teleport.windows": list("teleport.windows"),
    "teleport.remoteWindows": list("teleport.remoteWindows"),
    "teleport.icon": call("teleport.icon"),
    "teleport.thumbnail": call("teleport.thumbnail"),
    "teleport.plan": async ({ spaceId, entry, move, files, sensitiveGroups }) =>
      planFromWk(await request<Json>("teleport.plan", { spaceId, entry: { id: entry?.id }, move, files, sensitiveGroups })),
    "teleport.run": ({ spaceId, plan, consent, runId }) => request("teleport.run", { spaceId, plan: { json: plan?.json }, consent, runId }),
    "teleport.sites": async ({ providerId }) => snake(await request("teleport.sites", { providerId })) as KvInventory,
    "teleport.remembered": async ({ providerId, spaceId }) => (await request<string[] | null>("teleport.remembered", { providerId, spaceId })) ?? null,
    "teleport.streamWindow": none("teleport.streamWindow"),

    "sharing.list": list("sharing.list"),
    "sharing.share": list("sharing.share"),
    "sharing.unshare": list("sharing.unshare"),

    "volume.overview": call("volume.overview"),
    "volume.storage": call("volume.storage"),
    "volume.storageSet": call("volume.storageSet"),
    "volume.mount": call("volume.mount"),
    "volume.unmount": call("volume.unmount"),
    "volume.approve": none("volume.approve"),
    "volume.deny": none("volume.deny"),
    "volume.revoke": none("volume.revoke"),
    "volume.resolve": none("volume.resolve"),
    "volume.reveal": none("volume.reveal"),
    "agents.setupDriver": list("agents.setupDriver"),

    "about.get": call("about.get"),
    "about.set": call("about.set"),
    "about.checkNow": call("about.checkNow"),
    "loginItem.get": call("loginItem.get"),
    "loginItem.set": call("loginItem.set"),
    "loginItem.openSettings": none("loginItem.openSettings"),
    "devices.get": call("devices.get"),
    "devices.enroll": call("devices.enroll"),
    "devices.checkEnrolled": async () => Boolean(await request("devices.checkEnrolled")),
    "devices.approve": none("devices.approve"),
    "devices.rename": none("devices.rename"),
    "devices.revoke": none("devices.revoke"),
    "devices.confirmMachine": none("devices.confirmMachine"),
    "storage.get": call("storage.get"),
    "storage.run": call("storage.run"),
    "notifications.list": list("notifications.list"),
    "notifications.markAllRead": none("notifications.markAllRead"),

    // Only known signals of fixed words leave the page; the host drops
    // them all while usage data is off.
    "telemetry.track": async ({ signals }) => {
      const clean = sanitizeSignals(signals);
      if (clean.length) await request("telemetry.track", { signals: clean });
      return null;
    },
    "spaces.usage": call("spaces.usage"),
    "spaces.windows": call("spaces.windows"),
    "stream.pip": list("stream.pip"),
    "spaces.thumbnail": call("spaces.thumbnail"),
    "spaces.chooseFiles": list("spaces.chooseFiles"),
    "spaces.droppedFiles": list("spaces.droppedFiles"),
    "spaces.sendFiles": list("spaces.sendFiles"),

    "host.setUp": host("host.setUp"),
    "host.action": host("host.action"),
    "host.openSettings": async ({ url }) => {
      if (!isSettingsPane(url)) throw new HostError("Only System Settings panes open here", "bad_args");
      await request("host.openSettings", { url });
      return null;
    },
  };
}

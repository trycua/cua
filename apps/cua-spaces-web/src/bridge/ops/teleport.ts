// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Teleport an app": the host operations the picker, its grid and the review
 * use. Every one is a command the native apps already run (the Tauri app's
 * `teleport_*` and window commands, the SwiftUI app's `TeleportModel`
 * sources); nothing here plans, seals or sends anything itself. The app
 * core decides each step (`flow.*`, `grid.*`), and the SDK re-checks the
 * consent the run carries.
 *
 * | Operation | Tauri command |
 * |---|---|
 * | `teleport.catalog` `{spaceId}` | `teleport_catalog` (SDK JSON, mapped as `entryFromCore` maps it) |
 * | `teleport.entryForPath` `{path}` | `teleport_entry_for_path` |
 * | `teleport.windows` | `list_open_windows` |
 * | `teleport.remoteWindows` `{spaceId}` | `list_remote_windows` |
 * | `teleport.icon` `{spaceId, icon}` | `teleport_app_icon` (this machine) / `space_app_icon` (the Space) |
 * | `teleport.thumbnail` `{spaceId, thumbnail}` | `capture_window_thumbnail` / `remote_window_thumbnail` |
 * | `teleport.plan` `{spaceId, entry, move, files, sensitiveGroups}` | `teleport_plan` |
 * | `teleport.run` `{spaceId, plan, consent, runId}` | `teleport_run` (progress on its channel, re-sent as `teleport.progress`) |
 * | `teleport.sites` `{providerId}` | none: the Keyvault inventory has no Tauri command |
 * | `teleport.remembered` `{providerId, spaceId}` | none: the SwiftUI app's remembered site choices are its own setting |
 * | `teleport.streamWindow` `{spaceId, spaceName, windowId, appName, title}` | `stream_remote_windows` |
 *
 * Results are the core's camelCase shapes (`contracts/teleport.ts`); the
 * Tauri commands answer the SDK's snake_case, mapped here as the Tauri
 * app's `entryFromCore`/`planFromCore` map it. The event
 * `{ type: "teleport.progress", runId, event }` carries each `RunEvent` of
 * the run with that `runId`.
 *
 * **Native hosts** (the Electron shell answers the Swift host's methods): (`WebUIBridge+Pages.swift`): the same names with the same
 * args onto `TeleportModel`'s sources, as `TeleportPickerSources.live` and
 * `TeleportModel.plan/confirm` call the SDK: `Teleport.catalog(options:
 * spaceHint)`, `catalogEntryForPath`, `listWindows`, `Space.windows`,
 * `appIconPng` / `Space.appIcons`, `captureWindowThumbnail` /
 * `Space.windowThumbnail`, `Teleport.plan`, `Teleport.run` (its
 * `RunEvents` as `teleport.progress` window events), the Keyvault client's
 * `inventory(app:)`, and the Stream rows' window pop-out. The host keeps
 * the SDK's entries and plans between steps; `ops/webkit-pages.ts` maps
 * its answers.
 */

import type {
  CatalogEntry,
  KvInventory,
  OpenWindow,
  PickerTileIcon,
  PickerTileThumbnail,
  RemoteWindow,
  RunEvent,
  RunReport,
  SensitiveGroup,
  TeleportConsent,
  TeleportMove,
  TeleportPlan,
} from "../contracts/teleport";

export interface TeleportOperations {
  /** Every app on this machine, classified for the Space's OS and CPU. */
  "teleport.catalog": { args: { spaceId: string }; result: CatalogEntry[] };
  /** The catalog row for an app bundle (an Open windows tile's app). */
  "teleport.entryForPath": { args: { path: string }; result: CatalogEntry };
  /** This machine's windows, front to back. */
  "teleport.windows": { args: Record<string, never>; result: OpenWindow[] };
  /** The Space's windows. */
  "teleport.remoteWindows": { args: { spaceId: string }; result: RemoteWindow[] };
  /** A tile's app icon as a `data:` URL, or null. */
  "teleport.icon": { args: { spaceId: string; icon: PickerTileIcon }; result: string | null };
  /** A tile's live window preview as a `data:` URL, or null. */
  "teleport.thumbnail": { args: { spaceId: string; thumbnail: PickerTileThumbnail }; result: string | null };
  /** What teleporting the app will install, send and import. */
  "teleport.plan": {
    args: { spaceId: string; entry: CatalogEntry; move: TeleportMove; files: string[]; sensitiveGroups: SensitiveGroup[] };
    result: TeleportPlan;
  };
  /** Runs a plan the user approved, with the consent `flow.consent` built.
   * Progress arrives as `teleport.progress` events carrying `runId`. */
  "teleport.run": { args: { spaceId: string; plan: TeleportPlan; consent: TeleportConsent; runId: string }; result: RunReport };
  /** A browser's sites with counts, from the Keyvault (never values). */
  "teleport.sites": { args: { providerId: string }; result: KvInventory };
  /** The sites sent last time to this Space from this browser (the review starts from them), or null. */
  "teleport.remembered": { args: { providerId: string; spaceId: string }; result: string[] | null };
  /** Streams one of the Space's windows onto this machine (From <Space>). */
  "teleport.streamWindow": {
    args: { spaceId: string; spaceName: string; windowId: string; appName: string; title: string };
    result: null;
  };
}

export type TeleportOpName = keyof TeleportOperations;
type Args<K extends TeleportOpName> = TeleportOperations[K]["args"];
type Result<K extends TeleportOpName> = TeleportOperations[K]["result"];
export type TeleportHandlers = { [K in TeleportOpName]: (args: Args<K>) => Promise<Result<K>> };

export const TELEPORT_OPERATIONS = [
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
] as const satisfies readonly TeleportOpName[];

/** A run moved on (the Tauri channel's `RunEvent`). */
export type TeleportHostEvent = { type: "teleport.progress"; runId: string; event: RunEvent };
export const TELEPORT_EVENTS = ["teleport.progress"] as const;

/** No Tauri command answers these (`TAURI_UNSUPPORTED`). */
export const TELEPORT_TAURI_UNSUPPORTED = ["teleport.sites", "teleport.remembered"] as const satisfies readonly TeleportOpName[];

/* ---- Coverage (`../coverage.ts`) ------------------------------------------ */

const row = <K extends TeleportOpName>(op: K, tauri: readonly string[], tauriUnsupported?: string) => ({
  webkit: { methods: [op] as const },
  tauri,
  ...(tauriUnsupported ? { unsupported: { tauri: tauriUnsupported } } : {}),
});

export const TELEPORT_COVERAGE = {
  "teleport.catalog": row("teleport.catalog", ["teleport_catalog"]),
  "teleport.entryForPath": row("teleport.entryForPath", ["teleport_entry_for_path"]),
  "teleport.windows": row("teleport.windows", ["list_open_windows"]),
  "teleport.remoteWindows": row("teleport.remoteWindows", ["list_remote_windows"]),
  "teleport.icon": row("teleport.icon", ["teleport_app_icon", "space_app_icon"]),
  "teleport.thumbnail": row("teleport.thumbnail", ["capture_window_thumbnail", "remote_window_thumbnail"]),
  "teleport.plan": row("teleport.plan", ["teleport_plan"]),
  "teleport.run": row("teleport.run", ["teleport_run"]),
  "teleport.sites": row("teleport.sites", [], "the Keyvault inventory has no Tauri command"),
  "teleport.remembered": row("teleport.remembered", [], "remembered site choices are the SwiftUI app's setting"),
  "teleport.streamWindow": row("teleport.streamWindow", ["stream_remote_windows"]),
} as const;

/* ---- The SDK's snake_case JSON (as `@trycua/cua/teleport` maps it) --------- */

type Json = Record<string, unknown>;
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const num = (v: unknown): number => (typeof v === "number" ? v : 0);
const list = (v: unknown): string[] => (Array.isArray(v) ? (v as string[]) : []);

/** `cua_teleport::ux::CatalogEntry` as the core's entry (`entryFromCore`). */
export function entryFromSdk(j: Json): CatalogEntry {
  const install = (j.install ?? null) as Json | null;
  const launch = (j.launch ?? null) as Json | null;
  const kind = install ? str(install.kind) : null;
  return {
    id: String(j.id),
    name: String(j.name),
    hostPath: str(j.host_path),
    hostAppId: str(j.host_app_id),
    version: str(j.version),
    capability: j.capability as CatalogEntry["capability"],
    reason: str(j.reason),
    moves: (j.moves as TeleportMove[]) ?? [],
    providerId: str(j.provider_id),
    sensitiveGroups: Array.isArray(j.sensitive_groups) ? (j.sensitive_groups as SensitiveGroup[]) : [],
    installSource: kind === "manifest" || kind === "image" || kind === "space" ? kind : null,
    installId: install && kind === "manifest" ? str(install.id) : null,
    installVersion: install && kind === "manifest" ? str(install.version) : null,
    launchBin: launch ? str(launch.bin) : null,
    lastUsedMs: typeof j.last_used_ms === "number" ? j.last_used_ms : null,
    json: JSON.stringify(j),
  };
}

const STEP_NAME: Record<string, string> = { install: "install", send_files: "files", import_state: "state", launch: "launch" };

function stepSummary(step: Json): string {
  switch (String(step.kind)) {
    case "install":
      return `Install ${list(step.ids).join(", ")} (pinned, verified)`;
    case "send_files":
      return `Send ${list(step.paths).length} item(s) to ~/Downloads/${String(step.subdir)}`;
    case "import_state":
      return `Import ${list(step.items).length} ${String(step.provider_id)} item(s)`;
    case "launch": {
      const files = list(step.files);
      return files.length ? `Open ${String(step.bin)} with ${files.length} item(s)` : `Open ${String(step.bin)}`;
    }
    default:
      return String(step.kind);
  }
}

/** `cua_teleport::ux::TeleportPlan` as the core's plan (`planFromCore`). */
export function planFromSdk(j: Json): TeleportPlan {
  return {
    app: entryFromSdk(j.app as Json),
    spaceId: String(j.space_id),
    moves: j.moves as TeleportMove,
    steps: ((j.steps as Json[]) ?? []).map((s) => ({ kind: STEP_NAME[String(s.kind)] ?? String(s.kind), summary: stepSummary(s) })),
    consent: ((j.consent as Json[]) ?? []).map((c) => ({
      kind: c.kind as TeleportPlan["consent"][number]["kind"],
      key: String(c.key),
      label: String(c.label),
      detail: String(c.detail),
      bytes: num(c.bytes),
      sensitive: Boolean(c.sensitive),
    })),
    sensitive: Boolean(j.sensitive),
    relayUnsealed: Boolean(j.relay_unsealed),
    totalBytes: num(j.total_bytes),
    warnings: list(j.warnings),
    json: JSON.stringify(j),
  };
}

export function runEventFromSdk(j: Json): RunEvent {
  return {
    step: num(j.step),
    steps: num(j.steps),
    kind: String(j.kind),
    phase: j.phase as RunEvent["phase"],
    detail: String(j.detail ?? ""),
    doneBytes: num(j.done_bytes),
    totalBytes: num(j.total_bytes),
  };
}

export function runReportFromSdk(j: Json): RunReport {
  return {
    appId: String(j.app_id),
    installed: list(j.installed),
    sent: list(j.sent),
    imported: list(j.imported),
    skipped: list(j.skipped),
    launched: Boolean(j.launched),
  };
}

/** The core's consent (camelCase) as `cua_teleport::ux::Consent`. */
export function consentToSdk(c: TeleportConsent): Json {
  return {
    approved: c.approved,
    acknowledge_sensitive: c.acknowledgeSensitive,
    save_to_keyvault: Boolean(c.saveToKeyvault),
    acknowledge_relay_plaintext: Boolean(c.acknowledgeRelayPlaintext),
    cookie_domains: c.cookieDomains ?? null,
    exclude: c.exclude ?? [],
    from_vault: c.fromVault ?? null,
    include_passwords: Boolean(c.includePasswords),
  };
}

/* ---- Hosts ------------------------------------------------------------------ */

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** Tauri 2's IPC channel without `@tauri-apps/api`: a callback id the
 * command's `Channel` argument serialises to. */
interface TauriInternals {
  transformCallback?: (callback: (message: unknown) => void, once?: boolean) => number;
}

function tauriChannel(internals: TauriInternals | undefined, onMessage: (m: Json) => void): string | undefined {
  const id = internals?.transformCallback?.((raw) => {
    // Tauri 2.1+ wraps each message with its index.
    const r = raw as { message?: unknown; index?: number; end?: boolean } | null;
    if (r && typeof r === "object" && "index" in r) {
      if (r.end || r.message === undefined) return;
      onMessage(r.message as Json);
    } else {
      onMessage(raw as Json);
    }
  });
  return id === undefined ? undefined : `__CHANNEL__:${id}`;
}

/** The Tauri app's commands, as `src/native/teleportApps.ts` and
 * `src/native/teleport.ts` call them. */
export function tauriTeleportOps(
  invoke: Invoke,
  /** `window.__TAURI_INTERNALS__`. */
  tauriInternals: unknown,
  emit: (e: TeleportHostEvent) => void,
): Omit<TeleportHandlers, "teleport.sites" | "teleport.remembered"> {
  const internals = tauriInternals as TauriInternals | undefined;
  return {
    "teleport.catalog": async ({ spaceId }) => ((await invoke<Json[] | null>("teleport_catalog", { spaceId })) ?? []).map(entryFromSdk),
    "teleport.entryForPath": async ({ path }) => entryFromSdk(await invoke<Json>("teleport_entry_for_path", { path })),
    "teleport.windows": async () => (await invoke<OpenWindow[] | null>("list_open_windows")) ?? [],
    "teleport.remoteWindows": async ({ spaceId }) => (await invoke<RemoteWindow[] | null>("list_remote_windows", { spaceId })) ?? [],
    "teleport.icon": async ({ spaceId, icon }) => {
      if (icon.kind === "host") return invoke<string | null>("teleport_app_icon", { path: icon.path, size: 64 });
      if (icon.kind === "guest") {
        return invoke<string | null>("space_app_icon", { spaceId, appName: icon.appName, appId: icon.appId, pid: icon.pid || null });
      }
      return null;
    },
    "teleport.thumbnail": async ({ spaceId, thumbnail: t }) => {
      if (t.kind === "host-window") return invoke<string | null>("capture_window_thumbnail", { windowId: t.windowId });
      if (t.kind === "guest-window") {
        return invoke<string | null>("remote_window_thumbnail", { spaceId, windowId: t.windowId, targetEpoch: t.epoch });
      }
      return null;
    },
    "teleport.plan": async ({ spaceId, entry, move, files, sensitiveGroups }) =>
      planFromSdk(
        await invoke<Json>("teleport_plan", {
          spaceId,
          entry: JSON.parse(entry.json),
          options: { moves: move, files, ...(sensitiveGroups.length ? { sensitive_groups: sensitiveGroups } : {}), launch: true },
        }),
      ),
    "teleport.run": async ({ spaceId, plan, consent, runId }) => {
      const onEvent = tauriChannel(internals, (m) => emit({ type: "teleport.progress", runId, event: runEventFromSdk(m) }));
      return runReportFromSdk(
        await invoke<Json>("teleport_run", { spaceId, plan: JSON.parse(plan.json), consent: consentToSdk(consent), onEvent }),
      );
    },
    "teleport.streamWindow": async ({ spaceId, spaceName, windowId, appName, title }) => {
      await invoke("stream_remote_windows", { spaceId, spaceName, windowId, appName, title, replica: false });
      return null;
    },
  };
}

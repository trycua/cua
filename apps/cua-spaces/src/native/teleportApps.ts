// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Teleport an app…" over the shell: a `TeleportHost` (the cua SDK's
 * headless picker contract, `@trycua/cua/teleport`) backed by the Tauri
 * commands that call the SDK core (`teleport_catalog`, `teleport_plan`,
 * `teleport_run`, …). Outside Tauri a fixture host keeps the flow
 * previewable and testable; it never reads this machine's apps and never
 * sends anything.
 */
import {
  type CatalogEntry,
  type Plan,
  type RunReport,
  type TeleportHost,
  entryFromCore,
  planFromCore,
  runEventFromCore,
  runReportFromCore,
} from "@trycua/cua/teleport";

import { hasTauri } from "./bridge";

type Json = Record<string, unknown>;

/** A parsed drop (`teleport_parse_drop`). */
export interface DropView {
  kind: "empty" | "app" | "files" | "url";
  apps: string[];
  files: string[];
  urls: string[];
}

export interface TeleportAppsBridge {
  readonly isNative: boolean;
  /** The picker host for one Space. */
  host(spaceId: string): TeleportHost;
  /** The catalog row for a dropped app bundle. */
  entryForPath(path: string): Promise<CatalogEntry>;
  /** Sorts dropped paths into apps, files and URLs. */
  parseDrop(items: string[]): Promise<DropView>;
  /** This machine's app icon at `path` (the SDK's `Teleport.appIconPng`,
   * cached there) as a `data:` URL; null when it has none. */
  hostIcon(path: string): Promise<string | null>;
}

const isApp = (p: string) => /\.(app|desktop|lnk)\/?$/i.test(p);

function fixtureEntry(id: string, name: string, capability: CatalogEntry["capability"], extra: Json = {}): CatalogEntry {
  return entryFromCore({
    id,
    name,
    host_path: `/Applications/${name}.app`,
    host_app_id: null,
    version: null,
    icon: null,
    capability,
    reason: capability === "unsupported" ? "no Linux build in the install manifest and no teleport provider for its state" : null,
    moves:
      capability === "full"
        ? ["app_only", "app_with_files", "app_with_state"]
        : capability === "install_only"
          ? ["app_only", "app_with_files"]
          : [],
    provider_id: capability === "full" ? id : null,
    install: capability === "install_only" ? { kind: "manifest", id, version: "1.0.0", license: "", arches: ["aarch64", "x86_64"] } : capability === "full" ? { kind: "image" } : null,
    launch: capability === "unsupported" ? null : { bin: id, args: [], terminal: false },
    last_used_ms: null,
    ...extra,
  });
}

/** The browser preview's catalog (clearly fixtures). */
export const FIXTURE_CATALOG: CatalogEntry[] = [
  fixtureEntry("firefox", "Firefox", "full"),
  fixtureEntry("vscode", "Visual Studio Code", "install_only"),
  fixtureEntry("com.apple.Safari", "Safari", "unsupported"),
];

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export function createFallbackTeleportAppsBridge(catalog: CatalogEntry[] = FIXTURE_CATALOG): TeleportAppsBridge {
  const host: TeleportHost = {
    catalog: async () => {
      await delay(50);
      return catalog;
    },
    plan: async (entry, options) =>
      planFromCore({
        app: JSON.parse(entry.json),
        space_id: "preview",
        moves: options.moves,
        steps: [{ kind: "launch", bin: entry.launchBin ?? entry.id, args: [], files: options.files, terminal: false }],
        consent: options.files.map((f) => ({ kind: "file", key: f, label: f, detail: "preview only", bytes: 0, sensitive: false })),
        sensitive: false,
        total_bytes: 0,
        warnings: ["Browser preview: nothing is installed or sent."],
      }),
    run: async (plan: Plan): Promise<RunReport> => {
      await delay(100);
      return { appId: plan.app.id, installed: [], sent: [], imported: [], skipped: [], launched: false };
    },
  };
  return {
    isNative: false,
    host: () => host,
    entryForPath: async (path) => {
      const name = path.replace(/\/+$/, "").split("/").pop()?.replace(/\.app$/i, "") ?? path;
      const hit = catalog.find((e) => e.name === name);
      if (!hit) throw new Error(`${path} is not a known app in the preview catalog`);
      return hit;
    },
    parseDrop: async (items) => {
      const apps = items.filter(isApp).map((p) => p.replace(/\/+$/, ""));
      const files = items.filter((p) => !isApp(p) && p.startsWith("/"));
      return { kind: apps.length ? "app" : files.length ? "files" : "empty", apps, files, urls: [] };
    },
    hostIcon: async () => null,
  };
}

export function createTauriTeleportAppsBridge(): TeleportAppsBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) => (await core).invoke<T>(command, args);
  return {
    isNative: true,
    host: (spaceId) => ({
      catalog: async () => (await invoke<Json[]>("teleport_catalog", { spaceId })).map(entryFromCore),
      plan: async (entry, options) =>
        planFromCore(
          await invoke<Json>("teleport_plan", {
            spaceId,
            entry: JSON.parse(entry.json),
            options: {
              moves: options.moves,
              files: options.files,
              ...(options.stateItems ? { state_items: options.stateItems } : {}),
              ...(options.sensitiveGroups?.length ? { sensitive_groups: options.sensitiveGroups } : {}),
              ...(options.scope ? { scope: options.scope === "tabs" ? "tabs_only" : "full_profile" } : {}),
              launch: options.launch ?? true,
            },
          }),
        ),
      run: async (plan, consent, onEvent) => {
        const { Channel } = await core;
        const channel = new Channel<Json>();
        channel.onmessage = (e) => onEvent(runEventFromCore(e));
        return runReportFromCore(
          await invoke<Json>("teleport_run", {
            spaceId,
            plan: JSON.parse(plan.json),
            consent: {
              approved: consent.approved,
              acknowledge_sensitive: consent.acknowledgeSensitive,
              save_to_keyvault: consent.saveToKeyvault,
              acknowledge_relay_plaintext: consent.acknowledgeRelayPlaintext,
            },
            onEvent: channel,
          }),
        );
      },
      icon: async (entry) =>
        entry.hostPath ? invoke<string | null>("teleport_app_icon", { path: entry.hostPath, size: 64 }) : null,
      chooseFiles: () => invoke<string[]>("teleport_choose_files"),
    }),
    entryForPath: async (path) => entryFromCore(await invoke<Json>("teleport_entry_for_path", { path })),
    parseDrop: (items) => invoke<DropView>("teleport_parse_drop", { items }),
    hostIcon: (path) => invoke<string | null>("teleport_app_icon", { path, size: 64 }),
  };
}

export function createTeleportAppsBridge(): TeleportAppsBridge {
  return hasTauri() ? createTauriTeleportAppsBridge() : createFallbackTeleportAppsBridge();
}

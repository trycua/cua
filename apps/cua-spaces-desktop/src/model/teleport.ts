// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Teleport an app" (the SwiftUI app's TeleportModel.swift, the parts the
// web page does not own): where the picker gets its windows, icons and
// previews (the SDK and its caches), the consent a run carries, and the
// entries and plans the bridge keeps between the page's steps. The page
// runs the picker's state machine itself, on the app core.
//
// Off a Mac the SDK has no window list, window previews or window drags
// (the Swift app has none there either): the picker's "Open windows" tab is
// empty, and the apps come from the catalog (`.desktop` entries on Linux,
// Start Menu shortcuts on Windows).
import type { Native } from "../native/load";
import type { AppModel } from "./app-model";
import type {
  AppOpenWindow,
  AppRemoteWindow,
  SpaceAppIconRequest,
  SpaceLike,
  SpaceWindow,
  TeleportCatalogEntry,
  TeleportConsent,
  TeleportLike,
  TeleportMove,
  TeleportPlan,
  TeleportRunReport,
  TeleportSensitiveGroup,
  TeleportWindow,
} from "../native/generated/index";

/** Where the picker's grid gets its windows, icons and previews. */
export interface TeleportPickerSources {
  /** This machine's windows, front to back. */
  openWindows(): Promise<AppOpenWindow[]>;
  /** The Space's windows. */
  remoteWindows(): Promise<AppRemoteWindow[]>;
  /** This machine's app icon (`Teleport.appIconPng`, SDK-cached). */
  hostIcon(path: string): Promise<Uint8Array | null>;
  /** A live preview of this machine's window (the window-drag source). */
  hostThumbnail(windowId: number): Promise<Uint8Array | null>;
  /** A live preview of the Space's window. */
  guestThumbnail(windowId: string, epoch: bigint): Promise<Uint8Array | null>;
}

const bytes = (b: ArrayBuffer | Uint8Array | null | undefined): Uint8Array | null =>
  b === null || b === undefined ? null : b instanceof Uint8Array ? b : new Uint8Array(b);

/** The SDK's: this machine's windows and icons, the Space's windows and previews, all through the SDK's caches. */
export function liveSources(teleport: TeleportLike | null, space: SpaceLike | null): TeleportPickerSources {
  return {
    async openWindows() {
      let windows: TeleportWindow[];
      try {
        windows = teleport?.listWindows() ?? [];
      } catch {
        // No window list on this OS (or no permission): an empty tab.
        windows = [];
      }
      return windows.map((w) => ({ windowId: w.windowId, appId: "", appName: w.appName, windowTitle: w.title, supported: true, bundlePath: w.bundlePath }));
    },
    async remoteWindows() {
      let windows: SpaceWindow[];
      try {
        windows = (await space?.windows(undefined)) ?? [];
      } catch {
        windows = [];
      }
      return windows.map((w) => ({
        id: w.windowId,
        appName: w.appName,
        title: w.title,
        visible: w.onScreen,
        appId: w.appId,
        targetEpoch: w.epoch,
        widthPx: w.bounds.length === 4 ? Math.max(0, w.bounds[2]!) : undefined,
        heightPx: w.bounds.length === 4 ? Math.max(0, w.bounds[3]!) : undefined,
        pid: w.pid > 0 ? w.pid : undefined,
      }));
    },
    async hostIcon(path) {
      return bytes(teleport?.appIconPng(path, 64));
    },
    async hostThumbnail(windowId) {
      try {
        return bytes(teleport?.captureWindowThumbnail(windowId, 480));
      } catch {
        return null;
      }
    },
    async guestThumbnail(windowId, epoch) {
      try {
        return bytes(await space?.windowThumbnail(windowId, epoch, 480));
      } catch {
        return null;
      }
    },
  };
}

/** The SDK's move for the page's word (anything else is the app alone). */
export function moveOf(native: Native, word: unknown): TeleportMove {
  switch (word) {
    case "app_with_files":
      return native.TeleportMove.AppWithFiles;
    case "app_with_state":
      return native.TeleportMove.AppWithState;
    default:
      return native.TeleportMove.AppOnly;
  }
}

/** The SDK's sensitive groups for the page's words (unknown words are dropped). */
export function groupsOf(native: Native, words: unknown): TeleportSensitiveGroup[] {
  if (!Array.isArray(words)) return [];
  const G = native.TeleportSensitiveGroup;
  return words.flatMap((w): TeleportSensitiveGroup[] => (w === "sign_ins" ? [G.SignIns] : w === "passwords" ? [G.Passwords] : w === "history" ? [G.History] : []));
}

const strings = (v: unknown): string[] | undefined => (Array.isArray(v) && v.every((s) => typeof s === "string") ? (v as string[]) : undefined);

/**
 * The consent the page sends, as the SDK's: through the core's consent
 * record, the same path `TeleportModel.confirm` takes, so every field (Save
 * to Keyvault included) reaches the run. `saveToKeyvault` defaults to false
 * on the SDK type, so leaving it out would silently drop the checkbox.
 */
export function teleportConsent(c: Record<string, unknown>): TeleportConsent {
  return {
    approved: c.approved === true,
    acknowledgeSensitive: c.acknowledgeSensitive === true,
    saveToKeyvault: c.saveToKeyvault === true,
    acknowledgeRelayPlaintext: c.acknowledgeRelayPlaintext === true,
    cookieDomains: strings(c.cookieDomains),
    exclude: strings(c.exclude) ?? [],
    fromVault: strings(c.fromVault),
    includePasswords: c.includePasswords === true,
  };
}

/** A run's report, as the page reads it (`RunReport`). */
export function runReport(r: TeleportRunReport) {
  return { appId: r.appId, installed: r.installed, sent: r.sent, imported: r.imported, skipped: r.skipped, launched: r.launched };
}

/** Icon or preview bytes as a `data:` URL (PNG, JPEG or SVG), or null. */
export function dataURL(data: Uint8Array | null | undefined): string | null {
  if (!data || data.length === 0) return null;
  let type = "application/octet-stream";
  if (data[0] === 0x89 && data[1] === 0x50 && data[2] === 0x4e && data[3] === 0x47) type = "image/png";
  else if (data[0] === 0xff && data[1] === 0xd8) type = "image/jpeg";
  else if (data[0] === 0x3c) type = "image/svg+xml";
  return `data:${type};base64,${Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString("base64")}`;
}

/** The most entries picked from windows between two catalog reads that are kept (the catalog itself replaces the lot). */
export const MAX_ENTRIES = 512;

/** A plan kept between the page's `teleport.plan` and `teleport.run`. */
export interface KeptPlan {
  json: string;
  plan: TeleportPlan;
  providerId: string | undefined;
}

/**
 * What the bridge keeps for Teleport: the SDK's entries and plans the page
 * names by id and by plan, as `TeleportModel` keeps them between its steps.
 * Bounded as the model's are: the entries are the latest catalog read (plus
 * the apps picked from a window since), and a Space keeps only its latest
 * plan, which a finished or failed run drops.
 */
export class TeleportCache {
  private entries = new Map<string, TeleportCatalogEntry>();
  private readonly plans = new Map<string, KeptPlan>();

  /** The latest read replaces the last. */
  setCatalog(catalog: TeleportCatalogEntry[]): void {
    this.entries = new Map(catalog.map((e) => [e.id, e]));
  }

  /** An app picked from a window joins the catalog's. */
  addEntry(entry: TeleportCatalogEntry): void {
    this.entries.delete(entry.id);
    this.entries.set(entry.id, entry);
    for (const old of this.entries.keys()) {
      if (this.entries.size <= MAX_ENTRIES) break;
      this.entries.delete(old);
    }
  }

  entry(id: string): TeleportCatalogEntry | undefined {
    return this.entries.get(id);
  }

  get entryCount(): number {
    return this.entries.size;
  }

  keepPlan(spaceId: string, kept: KeptPlan): void {
    this.plans.set(spaceId, kept);
  }

  plan(spaceId: string): KeptPlan | undefined {
    return this.plans.get(spaceId);
  }

  /** Finished, failed or cancelled: the plan is spent (a newer plan for the Space stays). */
  dropPlan(spaceId: string, json: string): void {
    if (this.plans.get(spaceId)?.json === json) this.plans.delete(spaceId);
  }

  get planCount(): number {
    return this.plans.size;
  }
}

/** The SDK's request for a Space app's icon, as the page names it. */
export function iconRequest(icon: Record<string, unknown>): SpaceAppIconRequest {
  const pid = icon.pid;
  return {
    appName: typeof icon.appName === "string" ? icon.appName : "",
    appId: typeof icon.appId === "string" ? icon.appId : "",
    pid: typeof pid === "number" && Number.isFinite(pid) && pid >= 0 ? Math.floor(pid) : 0,
  };
}

/** A Space's name, which the remembered choices are keyed by (the native review's `rememberKey`). */
export function spaceName(model: AppModel, id: string): string {
  return model.spaces.find((s) => s.id === id)?.name ?? id;
}

/**
 * Remembers the sites sent now (to this Space, from this app) for the next
 * review, as `TeleportModel.rememberChoice` does: only a choice of sites from
 * the live app, never the Keyvault as the source.
 */
export function rememberChoice(model: AppModel, providerId: string | undefined, spaceId: string, consent: Record<string, unknown>): void {
  const domains = strings(consent.cookieDomains);
  if (!providerId || !domains || Array.isArray(consent.fromVault)) return;
  const key = model.native.appReviewRememberKey(providerId, spaceName(model, spaceId));
  model.settings.teleportChoices = model.native.appReviewRemember(model.settings.teleportChoices, key, domains);
  model.saveSettings();
}

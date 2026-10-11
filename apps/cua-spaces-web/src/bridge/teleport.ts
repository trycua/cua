// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Teleport an app" for one Space, as the SwiftUI app's `TeleportModel`
 * runs it: the app core's picker state machine (`flow.*`) and grid
 * (`grid.*`) over the host's catalog, windows, plan and run
 * (`ops/teleport.ts`). The page only draws what the core returns and sends
 * the user's input back; the core decides every step and the review gate,
 * and the SDK re-checks the consent the run carries.
 *
 * Each core call below is a function the parity harness answers through
 * (`parity-teleport.ts`), so the transcript the page produces is the one
 * the Rust core, the Tauri app and the SwiftUI app match.
 */

import { useContext, useSyncExternalStore } from "react";
import type { DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { KeyvaultOverview, VaultAction, VaultSource, VaultState, VaultView } from "./contracts/keyvault";
import type {
  CatalogEntry,
  EntrySection,
  OpenWindow,
  PickerEvent,
  PickerFrame,
  PickerGrid,
  PickerGridPrimary,
  PickerGridTab,
  PickerGridTabItem,
  PickerState,
  PickerTile,
  PickerTileIcon,
  PickerTileThumbnail,
  RemoteWindow,
  ReviewView,
  SensitiveGroup,
  SensitiveOption,
  TeleportConsent,
} from "./contracts/teleport";
import type { HostEvent } from "./protocol";
import type { BridgeStore } from "./store";

/* ---- The core's picker (`teleport::flow`) --------------------------------- */

export const pickerInitial = (core: CoreClient, spaceName: string) => core.call<PickerState>("flow.initial", { spaceName });
export const pickerReduce = (core: CoreClient, state: PickerState, event: PickerEvent) =>
  core.call<PickerState>("flow.reduce", { state, event });
export const pickerSections = (core: CoreClient, state: PickerState) => core.call<EntrySection[]>("flow.sections", { state });
export const pickerReview = (core: CoreClient, state: PickerState) => core.call<ReviewView | null>("flow.review", { state });
export const pickerCanPlan = (core: CoreClient, state: PickerState) => core.call<boolean>("flow.canPlan", { state });
export const pickerProgress = (core: CoreClient, state: PickerState) => core.call<number>("flow.progress", { state });
export const pickerStatus = (core: CoreClient, state: PickerState) => core.call<string | null>("flow.status", { state });
export const pickerSensitiveOptions = (core: CoreClient, state: PickerState) =>
  core.call<SensitiveOption[]>("flow.sensitiveOptions", { state });
export const pickerPlanSensitive = (core: CoreClient, state: PickerState) =>
  core.call<SensitiveGroup[]>("flow.planSensitive", { state });
export const pickerConsent = (core: CoreClient, state: PickerState) => core.call<TeleportConsent>("flow.consent", { state });

/** Everything a picker frame draws. */
export function pickerFrame(core: CoreClient, state: PickerState): PickerFrame {
  return {
    sections: pickerSections(core, state),
    review: pickerReview(core, state),
    canPlan: pickerCanPlan(core, state),
    progress: pickerProgress(core, state),
    status: pickerStatus(core, state),
    sensitive: pickerSensitiveOptions(core, state),
    planSensitive: pickerPlanSensitive(core, state),
  };
}

/* ---- The core's grid (`teleport::grid`) ----------------------------------- */

export const gridTabs = (core: CoreClient, spaceName: string) => core.call<PickerGridTabItem[]>("grid.tabs", { spaceName });
export const appGrid = (core: CoreClient, state: PickerState, windows: OpenWindow[]) =>
  core.call<PickerGrid>("grid.apps", { state, windows });
export const windowGrid = (core: CoreClient, windows: OpenWindow[], query: string, selected: string | null) =>
  core.call<PickerGrid>("grid.windows", { windows, query, selected });
export const remoteGrid = (core: CoreClient, windows: RemoteWindow[], query: string, selected: string | null) =>
  core.call<PickerGrid>("grid.remote", { windows, query, selected });
export const gridStep = (core: CoreClient, grid: PickerGrid, selected: string | null, delta: number) =>
  core.call<string | null>("grid.step", { grid, selected, delta });
export const gridPrimary = (core: CoreClient, tab: PickerGridTab, spaceName: string, grid: PickerGrid) =>
  core.call<PickerGridPrimary>("grid.primary", { tab, spaceName, grid });

/* ---- The session ------------------------------------------------------------ */

/** Columns of the grid, for the up and down arrows (the SwiftUI grid's). */
export const GRID_COLUMNS = 3;

export interface TeleportSession {
  spaceId: string;
  spaceName: string;
  /** The core's picker state. */
  state: PickerState;
  /** What the core says the step draws. */
  frame: PickerFrame;
  tab: PickerGridTab;
  tabs: PickerGridTabItem[];
  /** The active tab's tiles and primary button. */
  grid: PickerGrid;
  primary: PickerGridPrimary;
  /** The active tab's search text. */
  query: string;
  /** Why the Space's windows could not be listed. */
  remoteError: string | null;
  /** The browser's sites are being read for the review. */
  readingSites: boolean;
  /** The app's saved Keyvault items as the review lists them (the Keyvault
   * as the source, once read); null while the live app is the source. */
  vault: { view: VaultView; query: string } | null;
}

/** The Keyvault as the review lists it: what a teleport can send, with the
 * list's own state (search, selection, open groups). */
interface ReviewVault {
  overview: KeyvaultOverview;
  state: VaultState;
}

interface Session {
  spaceId: string;
  spaceName: string;
  state: PickerState;
  tab: PickerGridTab;
  windowQuery: string;
  windowSelected: string | null;
  remoteSelected: string | null;
  openWindows: OpenWindow[] | null;
  remoteWindows: RemoteWindow[] | null;
  remoteError: string | null;
  readingSites: boolean;
  vault: ReviewVault | null;
  runId: string | null;
}

/** An image the grid shows (an icon or a live preview), by what it asks for. */
export const iconKey = (icon: PickerTileIcon): string =>
  icon.kind === "host" ? `host:${icon.path}` : icon.kind === "guest" ? `guest:${icon.appName.toLowerCase()}:${icon.appId.toLowerCase()}` : "";
export const thumbnailKey = (t: PickerTileThumbnail): string =>
  t.kind === "host-window" ? `window:${t.windowId}` : t.kind === "guest-window" ? `guest-window:${t.windowId}@${t.epoch}` : "";

/** Most icons and previews kept at once: each is a base64 data URL, and a
 * guest window's key moves with its epoch. The oldest go first. */
export const MAX_IMAGES = 160;

/** Reads from the host at once. A catalog has hundreds of apps; asking for
 * every icon together queues them all on the host and they time out. */
export const MAX_READS = 4;

/** The Keyvault as the review lists it: saved passwords are never delivered
 * (they sign in through site login), so they are not offered here. */
const reviewOverview = (o: KeyvaultOverview): KeyvaultOverview => ({ ...o, items: o.items.filter((i) => i.kind !== "password") });

const words = (e: unknown) => (e instanceof Error ? e.message : String(e));
let runs = 0;

export class TeleportStore {
  private session: Session | null = null;
  /** Bumped by every open and close: answers for an older picker are dropped. */
  private generation = 0;
  private view: TeleportSession | null = null;
  private images = new Map<string, string | null>();
  private imagesView: ReadonlyMap<string, string | null> = new Map();
  private asked = new Set<string>();
  private reading = 0;
  private waiting: (() => void)[] = [];
  private listeners = new Set<() => void>();
  private readonly off: () => void;

  constructor(
    readonly adapter: DataAdapter,
    readonly core: CoreClient,
  ) {
    this.off = adapter.subscribe((e) => this.onEvent(e));
  }

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };
  getSession = () => this.view;
  getImages = () => this.imagesView;

  private publish(): void {
    const s = this.session;
    this.view = s ? this.compose(s) : null;
    for (const l of [...this.listeners]) l();
  }

  private compose(s: Session): TeleportSession {
    const grid = this.gridOf(s);
    const frame = pickerFrame(this.core, s.state);
    return {
      spaceId: s.spaceId,
      spaceName: s.spaceName,
      state: s.state,
      frame,
      tab: s.tab,
      tabs: gridTabs(this.core, s.spaceName),
      grid,
      primary: gridPrimary(this.core, s.tab, s.spaceName, grid),
      query: s.tab === "apps" ? s.state.query : s.windowQuery,
      remoteError: s.remoteError,
      readingSites: s.readingSites,
      vault:
        s.vault && frame.review?.source === "vault"
          ? {
              view: this.core.call<VaultView>("keyvault.vaultView", { overview: s.vault.overview, state: s.vault.state, now: Date.now() }),
              query: s.vault.state.query,
            }
          : null,
    };
  }

  private gridOf(s: Session): PickerGrid {
    switch (s.tab) {
      case "apps":
        return appGrid(this.core, s.state, s.openWindows ?? []);
      case "windows":
        return windowGrid(this.core, s.openWindows ?? [], s.windowQuery, s.windowSelected);
      case "space":
        return remoteGrid(this.core, s.remoteWindows ?? [], s.windowQuery, s.remoteSelected);
    }
  }

  private patch(p: Partial<Session>): void {
    if (!this.session) return;
    this.session = { ...this.session, ...p };
    this.publish();
  }

  /** One picker event through the core. */
  send(event: PickerEvent): void {
    const s = this.session;
    if (!s) return;
    this.patch({ state: pickerReduce(this.core, s.state, event) });
  }

  /** Opens the picker for a Space and reads the catalog and both window
   * lists; at `entry` (an app dropped on the Space's well, with the
   * `files` dropped beside it) when given. */
  open(space: { id: string; name: string }, entry?: CatalogEntry, files?: string[]): void {
    if (this.core.status !== "ready") throw new Error("Teleport needs the app core, which isn't loaded");
    const state = pickerInitial(this.core, space.name);
    this.forgetImages();
    this.session = {
      spaceId: space.id,
      spaceName: space.name,
      state,
      tab: "apps",
      windowQuery: "",
      windowSelected: null,
      remoteSelected: null,
      openWindows: null,
      remoteWindows: null,
      remoteError: null,
      readingSites: false,
      vault: null,
      runId: null,
    };
    this.publish();
    const opened = ++this.generation;
    const live = () => this.generation === opened;
    if (entry) this.send({ type: "preselect", entry, ...(files?.length ? { files } : {}) });
    else
      void this.adapter.call("teleport.catalog", { spaceId: space.id }).then(
        (entries) => live() && this.send({ type: "loaded", entries }),
        (e) => live() && this.send({ type: "failed", message: words(e) }),
      );
    void this.adapter.call("teleport.windows", {}).then(
      (openWindows) => live() && this.patch({ openWindows }),
      () => live() && this.patch({ openWindows: [] }),
    );
    void this.adapter.call("teleport.remoteWindows", { spaceId: space.id }).then(
      (remoteWindows) => live() && this.patch({ remoteWindows }),
      (e) => live() && this.patch({ remoteWindows: [], remoteError: words(e) }),
    );
  }

  close(): void {
    this.generation++;
    this.session = null;
    this.forgetImages();
    this.publish();
  }

  /** Drops every icon and preview (and what was asked for): a closed or
   * reopened picker starts empty, so they never outlive it. */
  private forgetImages(): void {
    this.waiting = [];
    if (this.images.size === 0 && this.asked.size === 0) return;
    this.images.clear();
    this.asked.clear();
    this.imagesView = new Map();
  }

  setTab(tab: PickerGridTab): void {
    this.patch({ tab, windowQuery: "" });
  }

  setQuery(text: string): void {
    if (this.session?.tab === "apps") this.send({ type: "query", query: text });
    else this.patch({ windowQuery: text });
  }

  select(id: string): void {
    const tab = this.session?.tab;
    if (tab === "apps") this.send({ type: "select", id });
    else if (tab === "windows") this.patch({ windowSelected: id });
    else if (tab === "space") this.patch({ remoteSelected: id });
  }

  /** Arrow keys: `delta` tiles (a row is `GRID_COLUMNS`). */
  step(delta: number): void {
    const s = this.session;
    if (!s || !this.view) return;
    const current = s.tab === "apps" ? s.state.selectedId : s.tab === "windows" ? s.windowSelected : s.remoteSelected;
    const next = gridStep(this.core, this.view.grid, current, delta);
    if (next) this.select(next);
  }

  /** Opens a tile: the app's options (Apps, Open windows) or the Space's
   * window streamed here (From <Space>). Without `tile`, the selected one. */
  async activate(tile?: PickerTile): Promise<void> {
    const s = this.session;
    const t = tile ?? this.view?.grid.sections.flatMap((x) => x.tiles).find((x) => x.selected);
    if (!s || !t || t.disabled) return;
    this.select(t.id);
    if (s.tab === "apps") {
      this.send({ type: "choose", id: t.id });
    } else if (s.tab === "windows") {
      const path = s.openWindows?.find((w) => String(w.windowId) === t.id)?.bundlePath;
      if (!path) return;
      try {
        const entry = await this.adapter.call("teleport.entryForPath", { path });
        this.patch({ tab: "apps" });
        this.send({ type: "preselect", entry });
      } catch (e) {
        this.send({ type: "failed", message: words(e) });
      }
    } else {
      const w = s.remoteWindows?.find((x) => x.id === t.id);
      await this.adapter.call("teleport.streamWindow", {
        spaceId: s.spaceId,
        spaceName: s.spaceName,
        windowId: t.id,
        appName: w?.appName ?? "",
        title: w?.title ?? t.title,
      });
      this.close();
    }
  }

  /** Plans the chosen app, then reads what the review lets the user choose. */
  async plan(): Promise<void> {
    const s = this.session;
    if (!s || !this.view?.frame.canPlan || !s.state.entry || !s.state.move) return;
    const { entry, move, files } = s.state;
    const sensitiveGroups = this.view.frame.planSensitive;
    this.send({ type: "plan" });
    const opened = this.generation;
    try {
      const plan = await this.adapter.call("teleport.plan", { spaceId: s.spaceId, entry, move, files, sensitiveGroups });
      if (this.generation !== opened) return;
      this.send({ type: "planned", plan });
      await this.loadChoices();
    } catch (e) {
      if (this.generation === opened) this.send({ type: "failed", message: words(e) });
    }
  }

  /** What the review lets the user choose, read as `TeleportModel.loadChoices`
   * does: the app's saved Keyvault items (counts only, no Touch ID) and, for a
   * browser that sends its cookies, its sites with counts (nothing decrypted;
   * the host asks for Touch ID to show names), starting from the sites sent
   * last time to this Space. */
  private async loadChoices(): Promise<void> {
    const s = this.session;
    const provider = s?.state.entry?.providerId;
    if (!s || !provider) return;
    const opened = this.generation;
    const live = () => this.generation === opened;
    const needsDomains = Boolean(this.view?.frame.review?.needsDomains);
    await this.loadVault(provider, live);
    if (!live() || !needsDomains) return;
    this.patch({ readingSites: true });
    try {
      const inventory = await this.adapter.call("teleport.sites", { providerId: provider });
      // Not asked of every host: no remembered choice is a fresh start.
      const remembered = await this.adapter.call("teleport.remembered", { providerId: provider, spaceId: s.spaceId }).catch(() => null);
      if (!live()) return;
      this.patch({ readingSites: false });
      this.send({ type: "domains-loaded", inventory, remembered });
    } catch {
      if (!live()) return;
      this.patch({ readingSites: false });
      this.send({ type: "domains-failed" });
    }
  }

  /** The saved items a teleport can send for `provider`: passwords are never
   * listed here (they sign in through site login), and the core counts the
   * rest. Without a Keyvault the review simply sends from the app. */
  private async loadVault(provider: string, live: () => boolean): Promise<void> {
    try {
      const overview = reviewOverview(await this.adapter.call("keyvault.overview", {}));
      const source = this.core.call<VaultSource>("keyvault.vaultSource", { overview, providerId: provider });
      if (!live()) return;
      this.patch({ vault: { overview, state: { query: "", selected: source.ids, expanded: [provider], app: provider } } });
      this.send({
        type: "vault-items",
        count: source.count,
        newestMs: source.newestMs,
        nowMs: Date.now(),
        selected: source.ids,
        passwordIds: source.passwordIds,
      });
    } catch {
      /* no Keyvault: nothing saved to send from */
    }
  }

  /** Where the review sends from. The saved items' names need Touch ID the
   * first time (the Keyvault's browse window). */
  async sendFrom(source: "live" | "vault"): Promise<void> {
    this.send({ type: "send-from", source });
    if (source === "vault" && !this.session?.vault?.overview.namesVisible) await this.showNames();
  }

  /** Shows the saved items' names (the daemon asks for Touch ID). */
  async showNames(): Promise<void> {
    if (!this.session?.vault) return;
    const opened = this.generation;
    try {
      const overview = reviewOverview(await this.adapter.call("keyvault.showItems", {}));
      const now = this.session?.vault;
      if (this.generation === opened && now) this.patch({ vault: { ...now, overview } });
    } catch {
      /* Touch ID declined: the names stay hidden, the list says so */
    }
  }

  /** Search, select and open groups in the saved items; the core keeps the
   * ones to send. */
  sendVault(action: VaultAction): void {
    const vault = this.session?.vault;
    if (!vault) return;
    const state = this.core.call<VaultState>("keyvault.vaultReduce", { overview: vault.overview, state: vault.state, action });
    this.patch({ vault: { ...vault, state } });
    this.send({ type: "vault-selection", selected: state.selected });
  }

  /** Runs the reviewed plan with the consent the core built from the review. */
  async confirm(): Promise<void> {
    const s = this.session;
    const plan = s?.state.plan;
    if (!s || !plan || !this.view?.frame.review?.canConfirm) return;
    const consent = pickerConsent(this.core, s.state);
    const runId = `run-${++runs}`;
    this.patch({ runId });
    this.send({ type: "confirm" });
    try {
      const report = await this.adapter.call("teleport.run", { spaceId: s.spaceId, plan, consent, runId });
      if (this.session?.runId === runId) this.send({ type: "finished", report });
    } catch (e) {
      if (this.session?.runId === runId) this.send({ type: "failed", message: words(e) });
    }
  }

  private onEvent(e: HostEvent): void {
    if (e.type === "teleport.progress" && this.session && e.runId === this.session.runId) {
      this.send({ type: "progress", event: e.event });
    }
  }

  /** A tile's icon or preview, read once from the host. */
  loadImage(key: string, read: () => Promise<string | null>): void {
    if (!key || this.asked.has(key)) return;
    this.asked.add(key);
    const opened = this.generation;
    void this.queued(opened, read).then(
      (url) => {
        // The picker closed (or reopened) while it was read.
        if (this.generation !== opened) return;
        this.images.set(key, url);
        // Oldest first (a Map keeps insertion order); asked again if shown again.
        for (const old of this.images.keys()) {
          if (this.images.size <= MAX_IMAGES) break;
          this.images.delete(old);
          this.asked.delete(old);
        }
        this.imagesView = new Map(this.images);
        for (const l of [...this.listeners]) l();
      },
      () => this.asked.delete(key),
    );
  }

  /** `read`, when fewer than `MAX_READS` are in flight; never for a picker that closed meanwhile. */
  private queued(opened: number, read: () => Promise<string | null>): Promise<string | null> {
    return new Promise((resolve, reject) => {
      const start = () => {
        if (this.generation !== opened) return reject(new Error("picker closed"));
        this.reading++;
        read()
          .then(resolve, reject)
          .finally(() => {
            this.reading--;
            this.waiting.shift()?.();
          });
      };
      if (this.reading < MAX_READS) start();
      else this.waiting.push(start);
    });
  }

  icon(tile: PickerTile): void {
    const s = this.session;
    if (s) this.loadImage(iconKey(tile.icon), () => this.adapter.call("teleport.icon", { spaceId: s.spaceId, icon: tile.icon }));
  }

  thumbnail(tile: PickerTile): void {
    const s = this.session;
    if (s) {
      this.loadImage(thumbnailKey(tile.thumbnail), () =>
        this.adapter.call("teleport.thumbnail", { spaceId: s.spaceId, thumbnail: tile.thumbnail }),
      );
    }
  }

  /** "512 B", "1.5 KB", "12 MB" (`flow.formatBytes`). */
  bytes(n: number): string {
    return this.core.call<string>("flow.formatBytes", { n });
  }

  /* ---- parity harness (parity-teleport.ts) ---- */

  /** Draws this picker state, as if the host and the user had produced it. */
  show(spaceName: string, state: PickerState): void {
    this.showGrid(spaceName, { tab: "apps", state, windows: [] });
  }

  /** Draws one tab of the grid. */
  showGrid(
    spaceName: string,
    g: { tab: PickerGridTab; state?: PickerState; windows?: OpenWindow[]; remote?: RemoteWindow[]; query?: string; selected?: string | null },
  ): void {
    this.session = {
      spaceId: "parity",
      spaceName,
      state: g.state ?? pickerReduce(this.core, pickerInitial(this.core, spaceName), { type: "loaded", entries: [] }),
      tab: g.tab,
      windowQuery: g.query ?? "",
      windowSelected: g.tab === "windows" ? (g.selected ?? null) : null,
      remoteSelected: g.tab === "space" ? (g.selected ?? null) : null,
      openWindows: g.windows ?? [],
      remoteWindows: g.remote ?? [],
      remoteError: null,
      readingSites: false,
      vault: null,
      runId: null,
    };
    this.publish();
  }

  dispose(): void {
    this.off();
    this.listeners.clear();
  }
}

const stores = new WeakMap<BridgeStore, TeleportStore>();

/** The teleport session of a bridge store (one per store). */
export function teleportStore(store: BridgeStore): TeleportStore {
  let t = stores.get(store);
  if (!t) {
    t = new TeleportStore(store.adapter, store.core);
    stores.set(store, t);
  }
  return t;
}

const noop = () => () => {};
const EMPTY: ReadonlyMap<string, string | null> = new Map();

export interface TeleportHook {
  /** The open picker, or null. */
  session: TeleportSession | null;
  /** Icons and previews read so far (`iconKey`, `thumbnailKey`). */
  images: ReadonlyMap<string, string | null>;
  /** The store, once the bridge is ready (actions). */
  teleport: TeleportStore | null;
  /** Opens the picker for a Space. */
  open(space: { id: string; name: string }): Promise<void>;
}

/** "Teleport an app": the open picker and its actions. */
export function useTeleport(): TeleportHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("useTeleport needs a <BridgeProvider> above it");
  const t = ctx.store ? teleportStore(ctx.store) : null;
  const session = useSyncExternalStore(t ? t.subscribe : noop, () => t?.getSession() ?? null, () => null);
  const images = useSyncExternalStore(t ? t.subscribe : noop, () => t?.getImages() ?? EMPTY, () => EMPTY);
  return {
    session,
    images,
    teleport: t,
    open: (space) => ctx.ready.then((s) => teleportStore(s).open(space)),
  };
}

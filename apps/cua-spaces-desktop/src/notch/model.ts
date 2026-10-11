// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The notch's state on the Electron side: the app core's notch reducer and
// view (the same calls the SwiftUI app's NotchModel and NotchController
// make), the timers its effects ask for, the layout for the helper's
// screens, window drags onto the notch, the drag permission and the tiles'
// thumbnails. It decides nothing itself; the helper only draws what it
// emits. Pure (no Electron), so it is tested with a fake core and clock.

import { dragKind, needsRow, overlayPhase, triggerPhase, wireLayout, wireView, type CoreNotchState, type CoreTriggerState, type NotchCore, type Tagged } from "./core";
import type { NotchInput, NotchLayout, NotchView, OsIcon, ScreenFacts, StateMessage } from "./protocol";

/** A window drag as the SDK's monitor reports it (`TeleportWindowDragEvent`). */
export interface WindowDragEvent {
  phase: string;
  x: number;
  y: number;
  window?: { windowId: number; appName: string } | null;
  app?: { name: string } | null;
  startFrame?: unknown;
  frame?: unknown;
}

/** The SDK's teleport object (`Cua.teleport()`): window drags and captures. */
export interface NotchTeleport {
  windowDragSupported(): boolean;
  windowDragPermitted(): boolean;
  requestWindowDragPermission(): boolean;
  /** May block for its event tap; an adapter can return a promise. */
  startWindowDrag(listener: { onEvent(event: WindowDragEvent): void }): { stop(): void } | Promise<{ stop(): void }>;
  captureWindowThumbnail(windowId: number, maxWidth: number | undefined): Uint8Array | ArrayBuffer | null | undefined;
}

/** Where the notch's clicks go (the app's windows and pages). */
export interface NotchActions {
  /** A tile: select that Space and show the main window. */
  openSpace(spaceId: string): void;
  /** The Spaces button: show the main window. */
  openMain(): void;
  /** The Settings button. */
  openSettings(): void;
  /** The live-access line: the Keyvault's Access page. */
  openAccess(): void;
  /** Its Dismiss: hide the indicator and the tiles' key (nothing is revoked). */
  dismissAccess(): void;
  /** A window drag committed to a Space: its teleport review for that app. */
  teleport(spaceId: string, app: unknown): void;
  /** Apps or files dropped on a tile: the teleport review for them. */
  drop(spaceId: string, paths: string[]): void;
}

export interface NotchHost {
  /** The cua-spaces-ffi bindings (the app core). */
  core: NotchCore;
  actions: NotchActions;
  /** A Space's screenshot (PNG or JPEG) for its tile. */
  thumbnail?(spaceId: string): Promise<Uint8Array | null | undefined>;
  /** Window drags onto the notch (none: the notch takes clicks and file drops only). */
  teleport?: NotchTeleport;
}

/** Timers and clocks; tests step them by hand. */
export interface NotchClock {
  /** Calls `fire` after `ms`; returns a cancel. */
  after(ms: number, fire: () => void): () => void;
  /** Monotonic milliseconds (the drag trigger's times). */
  monotonic(): number;
  /** Unix milliseconds (progress estimates). */
  now(): number;
}

export const systemClock: NotchClock = {
  after(ms, fire) {
    const t = setTimeout(fire, ms);
    return () => clearTimeout(t);
  },
  monotonic: () => performance.now(),
  now: () => Date.now(),
};

export interface NotchModelOptions {
  clock?: NotchClock;
  /** Opens a URL (System Settings). */
  openURL?(url: string): void;
  /** A forced look on one control (debug starts, as `CUA_SPACES_NOTCH_HIGHLIGHT`). */
  highlight?: string;
  /** Logs (default: console.warn). */
  log?(message: string): void;
}

/** What the helper gets: the state, and images. */
export interface NotchOutput {
  state(message: StateMessage): void;
  thumbnail(spaceId: string, image: Uint8Array | null): void;
  ghost(image: Uint8Array | null): void;
}

/** The System Settings pane for a permission line. */
export function settingsURL(pane: string): string {
  return pane === "accessibility"
    ? "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    : "x-apple.systempreferences:com.apple.preference.security";
}

const toBytes = (v: Uint8Array | ArrayBuffer | null | undefined): Uint8Array | null =>
  v ? (v instanceof Uint8Array ? v : new Uint8Array(v)) : null;

export class NotchModel {
  private state: CoreNotchState;
  private spaces: readonly unknown[] = [];
  private trigger: CoreTriggerState;
  private shown = true;
  private notchScreen?: ScreenFacts;
  private primaryTop = 0;
  private coreLayout?: unknown;
  private layout?: NotchLayout;
  private layoutRow = false;
  private dragDisplays: unknown[] = [];
  private draggedApp: unknown = null;
  private cancelTimer?: () => void;
  private cancelTick?: () => void;
  private monitor?: { stop(): void };
  private dragsRunning = false;
  private permissionPoll?: () => void;
  private thumbnailLoop?: () => void;
  private lastThumbnails = Number.NEGATIVE_INFINITY;
  private activityStart?: number;
  private estimateTick?: () => void;
  private lastState = "";
  private readonly clock: NotchClock;
  private readonly log: (message: string) => void;

  constructor(
    private readonly host: NotchHost,
    private readonly out: NotchOutput,
    private readonly options: NotchModelOptions = {},
  ) {
    this.clock = options.clock ?? systemClock;
    this.log = options.log ?? ((m) => console.warn(`[cua-spaces] notch: ${m}`));
    this.state = host.core.appNotchInitial();
    this.trigger = host.core.appDragTriggerInitial();
  }

  /** The core's motion and radii (the helper's hello). */
  motion() {
    const r = this.host.core.appNotchRadii();
    return { motion: this.host.core.appNotchMotion(), radii: { closed: r[0]!, open: r[1]! } };
  }

  /** The core's view, as the helper draws it. */
  view(): NotchView {
    const v = wireView(this.host.core.appNotchView(this.state, this.spaces));
    // A ring without real progress: the core's estimate from its start (or
    // from when it appeared), computed here so the helper needs no core.
    if (v.activity) {
      if (v.activity.permille === undefined) {
        this.activityStart ??= v.activity.startedAt ?? this.clock.now();
        const start = v.activity.startedAt ?? this.activityStart;
        v.activity.permille = this.host.core.appNotchEstimatedProgress(BigInt(Math.round(this.clock.now() - start)), v.activity.estimateMs);
      }
    } else {
      this.activityStart = undefined;
    }
    return v;
  }

  // MARK: - Inputs from the app

  /** The Spaces (the core's `AppSpace` records). */
  setSpaces(spaces: readonly unknown[]): void {
    this.spaces = spaces;
    this.changed();
  }

  /** The hotspot or a transfer changed (the indicator left of the notch). */
  setActivity(hotspot: boolean, transfer?: { sent?: number | bigint; total?: number | bigint } | null): void {
    const t = transfer ? { sent: big(transfer.sent), total: big(transfer.total) } : undefined;
    if (hotspot === this.state.hotspot && sameTransfer(t, this.state.transfer)) return;
    this.send({ tag: "Activity", inner: { hotspot, transfer: t } });
  }

  /** Keyvault sign-ins went live, changed or stopped (label less dismissed copies). */
  setKeyvault(label: string | null | undefined, signedIn: string[] = []): void {
    const l = label ?? undefined;
    if (l === this.state.keyvault && signedIn.join("\n") === this.state.signedIn.join("\n")) return;
    this.send({ tag: "Keyvault", inner: { label: l, signedIn } });
  }

  /** Shows or hides the notch (the "Spaces tab in the notch" setting). */
  setShown(shown: boolean): void {
    if (shown) this.show();
    else this.hide();
  }

  get isShown(): boolean {
    return this.shown;
  }

  private show(): void {
    if (!this.shown) {
      this.shown = true;
      this.send({ tag: "Visibility", inner: { shown: true } });
    }
    this.recheckPermission();
    this.changed();
  }

  private hide(): void {
    this.shown = false;
    this.send({ tag: "Visibility", inner: { shown: false } });
    this.monitor?.stop();
    this.monitor = undefined;
    this.dragsRunning = false;
    this.trigger = this.host.core.appDragTriggerInitial();
    this.cancelTick?.();
    this.cancelTick = undefined;
    this.permissionPoll?.();
    this.permissionPoll = undefined;
    this.stageChanged(false);
    this.changed();
  }

  // MARK: - Inputs from the helper

  /** Hover, clicks, Escape, a file drag over the notch, the search text. */
  input(e: NotchInput): void {
    switch (e.kind) {
      case "hoverEnter":
        return this.send({ tag: "HoverEnter" });
      case "hoverExit":
        return this.send({ tag: "HoverExit" });
      case "click":
        return this.send({ tag: "Click" });
      case "dismiss":
        return this.send({ tag: "Dismiss" });
      case "escape":
        return this.send({ tag: "Escape" });
      case "dropTargeted":
        return this.send({ tag: "DropTargeted", inner: { targeted: e.targeted } });
      case "search":
        if (e.query !== this.state.query) this.send({ tag: "Search", inner: { query: e.query } });
        return;
    }
  }

  /**
   * The helper's screens: lay out for the notch screen, else the primary
   * one (the SwiftUI app's `NotchScreens.screen()`: the notched display,
   * else the main one), so the helper always gets a layout when it has a
   * screen at all.
   */
  screens(notch: ScreenFacts | undefined, primary: ScreenFacts | undefined): void {
    notch ??= primary;
    this.notchScreen = notch;
    const p = primary ?? notch;
    if (notch && p) {
      this.primaryTop = p.frame.y + p.frame.height;
      // The primary first (it sets the top), then the notch screen: the
      // panel only shows there, so it is the one trigger display.
      const all = this.host.core.appDragDisplays(sameScreen(p, notch) ? [notch] : [p, notch]);
      this.dragDisplays = all.length > 0 ? [all[all.length - 1]] : [];
    }
    this.place();
    this.changed();
  }

  /** The permission line's button: registers the app for the permission and opens its pane. */
  openPermissionSettings(pane: string): void {
    try {
      this.host.teleport?.requestWindowDragPermission();
    } catch (error) {
      this.log(`could not request the window-drag permission: ${String(error)}`);
    }
    this.options.openURL?.(settingsURL(pane));
  }

  /** The panel finished opening or closing. */
  stageChanged(open: boolean): void {
    if (open) this.startThumbnails();
    else {
      this.thumbnailLoop?.();
      this.thumbnailLoop = undefined;
    }
  }

  // MARK: - The reducer

  /** Runs one event through the core and its effects. */
  send(event: Tagged): void {
    const t = this.host.core.appNotchReduce(this.state, event);
    this.state = t.state;
    for (const effect of t.effects) this.run(effect);
    this.changed();
  }

  private run(effect: Tagged): void {
    switch (effect.tag) {
      case "CancelTimers":
        this.cancelTimer?.();
        this.cancelTimer = undefined;
        return;
      case "StartDwell":
        return this.schedule(Number(effect.inner?.ms), { tag: "DwellElapsed" });
      case "StartClose":
        return this.schedule(Number(effect.inner?.ms), { tag: "CloseElapsed" });
      case "Drag": {
        const e = effect.inner?.effect as Tagged;
        if (e.tag === "Capture") {
          const id = e.inner?.windowId as number | undefined;
          const ghost = id === undefined || id === null ? null : this.capture(id);
          this.out.ghost(ghost);
          this.send({ tag: "Drag", inner: { event: { tag: "GhostReady", inner: { ghost: ghost ? "captured" : undefined } } } });
        } else if (e.tag === "Commit") {
          const spaceId = e.inner?.spaceId as string;
          this.host.actions.teleport(spaceId, this.draggedApp);
          this.draggedApp = null;
          this.out.ghost(null);
        }
        return;
      }
    }
  }

  private capture(windowId: number): Uint8Array | null {
    try {
      return toBytes(this.host.teleport?.captureWindowThumbnail(windowId, 320));
    } catch {
      return null;
    }
  }

  private schedule(ms: number, event: Tagged): void {
    this.cancelTimer?.();
    this.cancelTimer = this.clock.after(ms, () => {
      this.cancelTimer = undefined;
      this.send(event);
    });
  }

  // MARK: - Layout and the state the helper draws

  /** Re-reads the layout for the notch screen (when the row above the tiles comes or goes). */
  private place(): void {
    if (!this.notchScreen) return;
    this.layoutRow = needsRow(wireView(this.host.core.appNotchView(this.state, this.spaces)));
    this.coreLayout = this.host.core.appNotchLayout(this.notchScreen, this.layoutRow);
    this.layout = wireLayout(this.coreLayout);
  }

  /** Sends the state when it changed. */
  private changed(): void {
    const view = this.view();
    if (this.notchScreen && needsRow(view) !== this.layoutRow) this.place();
    if (view.hoverCue) this.refreshThumbnailsSoon();
    this.followEstimate(view);
    const message: StateMessage = {
      type: "state",
      view,
      query: this.state.query,
      layout: this.layout,
      shown: this.shown,
      dragging: overlayPhase(this.state.drag.phase) !== "idle",
      icons: this.icons(view),
      highlight: this.options.highlight,
    };
    const key = JSON.stringify(message);
    if (key === this.lastState) return;
    this.lastState = key;
    this.out.state(message);
  }

  /** Sends the last state again (a restarted helper). */
  resend(): void {
    this.lastState = "";
    this.changed();
  }

  private icons(view: NotchView): Record<string, OsIcon> {
    const icons: Record<string, OsIcon> = {};
    for (const t of view.tiles) {
      if (icons[t.symbol]) continue;
      const symbol = this.host.core.appOsIconSystemSymbol(t.symbol) ?? undefined;
      const svg = symbol ? undefined : (this.host.core.appOsIconSvg(t.symbol) ?? undefined);
      if (symbol || svg) icons[t.symbol] = { symbol, svg };
    }
    return icons;
  }

  /** While an estimated ring shows, its progress moves every 200 ms. */
  private followEstimate(view: NotchView): void {
    const estimating = this.shown && view.activity !== undefined && view.activity.symbol === undefined && view.activity.estimateMs > 0;
    if (estimating && !this.estimateTick) {
      const tick = () => {
        this.estimateTick = this.clock.after(200, () => {
          this.estimateTick = undefined;
          this.changed();
        });
      };
      tick();
    } else if (!estimating && this.estimateTick) {
      this.estimateTick();
      this.estimateTick = undefined;
    }
  }

  // MARK: - Thumbnails

  /** While open: every live tile's screenshot now, then every 3 s (at most 10 min). */
  private startThumbnails(): void {
    if (!this.host.thumbnail || this.thumbnailLoop) return;
    let rounds = 0;
    let cancel: (() => void) | undefined;
    let stopped = false;
    const round = () => {
      if (stopped || rounds++ >= 200) return;
      void this.refreshThumbnails().finally(() => {
        if (!stopped) cancel = this.clock.after(3000, round);
      });
    };
    this.thumbnailLoop = () => {
      stopped = true;
      cancel?.();
    };
    round();
  }

  /** The hover cue warms the thumbnails before the panel opens. */
  private refreshThumbnailsSoon(): void {
    if (!this.host.thumbnail || this.clock.now() - this.lastThumbnails <= 3000) return;
    void this.refreshThumbnails();
  }

  private async refreshThumbnails(): Promise<void> {
    const fetch = this.host.thumbnail;
    if (!fetch) return;
    this.lastThumbnails = this.clock.now();
    const open = { ...this.host.core.appNotchInitial(), open: true } as CoreNotchState;
    const live = wireView(this.host.core.appNotchView(open, this.spaces)).tiles.filter((t) => t.dropTarget && !t.dim);
    await Promise.all(
      live.map(async (t) => {
        try {
          const image = await fetch(t.id);
          if (image) this.out.thumbnail(t.id, image);
        } catch (error) {
          this.log(`no thumbnail for ${t.id}: ${String(error)}`);
        }
      }),
    );
  }

  // MARK: - Window drags onto the notch

  /**
   * Follows window drags. Detection needs the Accessibility permission;
   * without it the open panel says so (the core's line) and this checks
   * again when the app becomes active and every 2 s until it is granted.
   */
  recheckPermission(): void {
    const teleport = this.host.teleport;
    if (!this.shown || !teleport || this.dragsRunning || !safe(() => teleport.windowDragSupported(), false)) return;
    if (safe(() => teleport.windowDragPermitted(), false)) {
      this.dragsRunning = true;
      this.permissionPoll?.();
      this.permissionPoll = undefined;
      this.send({ tag: "DragPermission", inner: { granted: true } });
      void this.startDrags(teleport);
      return;
    }
    this.send({ tag: "DragPermission", inner: { granted: false } });
    this.pollPermission();
  }

  private async startDrags(teleport: NotchTeleport): Promise<void> {
    try {
      const monitor = await teleport.startWindowDrag({ onEvent: (event) => this.handleDrag(event) });
      // Hidden meanwhile: drop it.
      if (!this.shown || !this.dragsRunning || this.monitor) return monitor.stop();
      this.monitor = monitor;
    } catch (error) {
      this.log(`window-drag detection did not start: ${String(error)}`);
      if (!this.dragsRunning || this.monitor) return;
      this.dragsRunning = false;
      this.send({ tag: "DragPermission", inner: { granted: false } });
      this.pollPermission();
    }
  }

  private pollPermission(): void {
    if (!this.shown || this.permissionPoll) return;
    let cancel: (() => void) | undefined;
    let stopped = false;
    // Bounded by the app's life: stops once granted or when the notch hides.
    const tick = () => {
      cancel = this.clock.after(2000, () => {
        if (stopped) return;
        this.permissionPoll = undefined;
        this.recheckPermission();
        if (this.permissionPoll === undefined && !this.dragsRunning && this.shown) tick();
      });
      this.permissionPoll = () => {
        stopped = true;
        cancel?.();
      };
    };
    tick();
  }

  /** One window-drag event from the SDK's monitor. */
  handleDrag(event: WindowDragEvent): void {
    // Hidden: window drags are not followed (a late event is dropped).
    if (!this.shown) return;
    const now = this.clock.monotonic();
    if (event.phase === "start") this.draggedApp = event.app ?? null;
    if (event.phase !== "start" && event.frame) this.feed({ tag: "Frame", inner: { frame: event.frame } }, event);
    switch (event.phase) {
      case "start":
        this.feed(
          {
            tag: "Start",
            inner: {
              windowId: event.window?.windowId,
              appName: event.window?.appName ?? event.app?.name,
              x: event.x,
              y: event.y,
              tMs: BigInt(Math.round(now)),
              startFrame: event.startFrame ?? undefined,
              frame: event.frame ?? undefined,
            },
          },
          event,
        );
        break;
      case "move":
        this.feed({ tag: "Cursor", inner: { x: event.x, y: event.y, tMs: BigInt(Math.round(now)) } }, event);
        break;
      case "end":
        this.feed({ tag: "End", inner: { x: event.x, y: event.y, tMs: BigInt(Math.round(now)) } }, event);
        break;
      default:
        this.feed({ tag: "Cancel" }, event);
    }
  }

  /** The current drag's kind ("pending", "move", "resize"). */
  get dragKind(): string {
    return dragKind(this.trigger.kind);
  }

  /**
   * Runs one trigger event: its overlay events go to the notch (a drop
   * carries the tile under the cursor), the tiles are hit-tested while
   * expanded, and the dwell tick is scheduled.
   */
  private feed(e: Tagged, event: WindowDragEvent): void {
    const t = this.host.core.appDragTriggerApply(this.trigger, e, this.dragDisplays);
    this.trigger = t.state;
    for (const o of t.overlay) {
      if (o.tag === "Drop") this.send({ tag: "Drag", inner: { event: { tag: "Drop", inner: { spaceId: this.tileAt(event.x, event.y) } } } });
      else this.send({ tag: "Drag", inner: { event: o } });
    }
    if (triggerPhase(t.state.phase) === "expanded") {
      const tile = this.tileAt(event.x, event.y);
      this.send({ tag: "Drag", inner: { event: tile ? { tag: "Over", inner: { spaceId: tile } } : { tag: "Out" } } });
    }
    this.cancelTick?.();
    this.cancelTick = undefined;
    if (t.tickAtMs !== undefined && t.tickAtMs !== null) {
      const wait = Math.max(0, Number(t.tickAtMs) - this.clock.monotonic());
      this.cancelTick = this.clock.after(wait, () => {
        this.cancelTick = undefined;
        this.feed({ tag: "Tick", inner: { tMs: BigInt(Math.round(this.clock.monotonic())) } }, event);
      });
    }
  }

  /** The drop-target tile under a global top-left point (the core's hit test over the layout). */
  private tileAt(x: number, y: number): string | undefined {
    const coreView = this.host.core.appNotchView(this.state, this.spaces) as { tiles: unknown[] };
    const view = wireView(coreView);
    if (!this.coreLayout || view.phase !== "tiles") return undefined;
    return this.host.core.appNotchTileAt(this.coreLayout as NotchLayout, coreView.tiles, needsRow(view), x, this.primaryTop - y) ?? undefined;
  }

  /** Stops every timer and the drag monitor (the app quits). */
  dispose(): void {
    this.cancelTimer?.();
    this.cancelTick?.();
    this.permissionPoll?.();
    this.thumbnailLoop?.();
    this.estimateTick?.();
    this.monitor?.stop();
    this.monitor = undefined;
  }
}

const big = (v: number | bigint | undefined): bigint | undefined => (v === undefined ? undefined : BigInt(v));

function sameTransfer(a: { sent?: bigint; total?: bigint } | undefined, b: unknown): boolean {
  const o = b as { sent?: bigint | number; total?: bigint | number } | undefined | null;
  if (!a || !o) return !a && !o;
  return big(o.sent) === a.sent && big(o.total) === a.total;
}

function sameScreen(a: ScreenFacts, b: ScreenFacts): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function safe<T>(f: () => T, fallback: T): T {
  try {
    return f();
  } catch {
    return fallback;
  }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Teleport an app…" for this app: the SDK's picker host contract
 * (`@trycua/cua/teleport`: catalog, plan, run) driven by the app core's
 * state machine (`cua-spaces-app-core::teleport::flow`), the same one the
 * SwiftUI app runs. Only the async plumbing lives here.
 */
import type {
  CatalogEntry,
  Move,
  PickerEvent as SdkPickerEvent,
  PickerState,
  SensitiveGroup,
  TeleportHost,
} from "@trycua/cua/teleport";

import { core } from "../core";

export type { PickerState };

/** The core's picker events (a failure carries its cause's strings). */
export type PickerEvent =
  | Exclude<SdkPickerEvent, { type: "failed" }>
  | { type: "failed"; message: string; causeTexts?: string[]; causeInstalled?: boolean };

/** Strings a failure's cause carries (code, message, detail), for the
 * Keyvault's "needs the Cua app" prompt; the core decides what they mean. */
export function causeTexts(error: unknown, depth = 0): string[] {
  if (error == null || depth > 3) return [];
  if (typeof error === "string") return [error];
  if (typeof error !== "object") return [String(error)];
  const o = error as Record<string, unknown>;
  const out: string[] = [];
  for (const key of ["code", "kind", "tag", "message", "detail"]) {
    if (typeof o[key] === "string") out.push(o[key] as string);
  }
  if (o.error !== undefined) out.push(...causeTexts(o.error, depth + 1));
  if (o.cause !== undefined) out.push(...causeTexts(o.cause, depth + 1));
  if (typeof o.installed === "boolean" && o.open_url === "cua://keyvault") out.push("requires_cua_app");
  return out;
}

function failed(e: unknown): PickerEvent {
  const installed = typeof e === "object" && e !== null && (e as { installed?: unknown }).installed === true;
  return {
    type: "failed",
    message: e instanceof Error ? e.message : String(e),
    causeTexts: causeTexts(e),
    causeInstalled: installed,
  };
}

export function initialPicker(spaceName: string): PickerState {
  return core("flow.initial", { spaceName });
}

export function reducePicker(state: PickerState, event: PickerEvent): PickerState {
  return core("flow.reduce", { state, event });
}

export function sections(s: PickerState): { title: string; entries: CatalogEntry[] }[] {
  return core("flow.sections", { state: s });
}

export function canPlan(s: PickerState): boolean {
  return core("flow.canPlan", { state: s });
}

export function canConfirm(s: PickerState): boolean {
  return core("flow.canConfirm", { state: s });
}

/** One opt-in checkbox of the options step. */
export interface SensitiveOption {
  group: SensitiveGroup;
  label: string;
  detail: string;
  checked: boolean;
}

/** The options step's opt-ins ("Keep me signed in", "Saved passwords",
 * "Browsing history"), with the signed-in state move chosen. */
export function sensitiveOptions(s: PickerState): SensitiveOption[] {
  return core("flow.sensitiveOptions", { state: s });
}

/** The groups the plan asks for. */
export function planSensitive(s: PickerState): SensitiveGroup[] {
  return core("flow.planSensitive", { state: s });
}

export function progress(s: PickerState): number {
  return core("flow.progress", { state: s });
}

export function formatBytes(n: number): string {
  return core("flow.formatBytes", { n });
}

export function defaultMove(entry: CatalogEntry, files: readonly string[] = []): Move | null {
  return core("flow.defaultMove", { entry, files });
}

/** The picker bound to a host; React subscribes with useSyncExternalStore. */
export class TeleportPickerController {
  #state: PickerState;
  #listeners = new Set<() => void>();
  #generation = 0;
  readonly host: TeleportHost;

  constructor(host: TeleportHost, options: { spaceName: string; preselect?: { entry: CatalogEntry; files?: string[] } }) {
    this.host = host;
    this.#state = initialPicker(options.spaceName);
    if (options.preselect) {
      this.#state = reducePicker(this.#state, {
        type: "preselect",
        entry: options.preselect.entry,
        files: options.preselect.files ?? [],
      });
    }
  }

  get state(): PickerState {
    return this.#state;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  dispatch = (event: PickerEvent): void => {
    const next = reducePicker(this.#state, event);
    if (JSON.stringify(next) === JSON.stringify(this.#state)) return;
    this.#state = next;
    for (const l of [...this.#listeners]) l();
  };

  async load(): Promise<void> {
    const gen = ++this.#generation;
    try {
      const entries = await this.host.catalog();
      if (gen === this.#generation) this.dispatch({ type: "loaded", entries });
    } catch (e) {
      if (gen === this.#generation) this.dispatch(failed(e));
    }
  }

  choose(id?: string): void {
    this.dispatch(id === undefined ? { type: "choose" } : { type: "choose", id });
  }

  async chooseFiles(): Promise<void> {
    if (!this.host.chooseFiles) return;
    const files = await this.host.chooseFiles();
    if (files.length) this.dispatch({ type: "files", files });
  }

  async plan(): Promise<void> {
    const s = this.#state;
    if (s.step !== "options" || !canPlan(s) || !s.entry || !s.move) return;
    this.dispatch({ type: "plan" });
    try {
      const plan = await this.host.plan(s.entry, {
        moves: s.move,
        files: s.files,
        sensitiveGroups: planSensitive(s),
      });
      this.dispatch({ type: "planned", plan });
    } catch (e) {
      this.dispatch(failed(e));
    }
  }

  async confirm(): Promise<void> {
    const s = this.#state;
    if (s.step !== "consent" || !canConfirm(s) || !s.plan) return;
    const plan = s.plan;
    const consent = core<{
      approved: boolean;
      acknowledgeSensitive: boolean;
      saveToKeyvault: boolean;
      acknowledgeRelayPlaintext: boolean;
    }>("flow.consent", { state: s });
    this.dispatch({ type: "confirm" });
    try {
      const report = await this.host.run(plan, consent, (event) => this.dispatch({ type: "progress", event }));
      this.dispatch({ type: "finished", report });
    } catch (e) {
      this.dispatch(failed(e));
    }
  }
}

/** A picker tile's icon source (the core's `teleport::grid`). */
export type PickerTileIcon =
  | { kind: "host"; path: string }
  | { kind: "guest"; appName: string; appId: string; pid: number }
  | { kind: "none" };

/** A picker tile's live preview source. */
export type PickerTileThumbnail =
  | { kind: "host-window"; windowId: number }
  | { kind: "guest-window"; windowId: string; epoch: number }
  | { kind: "none" };

export interface PickerTile {
  id: string;
  title: string;
  help: string;
  disabled: boolean;
  selected: boolean;
  icon: PickerTileIcon;
  thumbnail: PickerTileThumbnail;
}

export interface PickerGrid {
  sections: { title: string; tiles: PickerTile[] }[];
  emptyText: string | null;
}

/** The Apps tab as tiles: each app previews its frontmost window. */
export function appGrid(
  s: PickerState,
  windows: { windowId: number; appId: string; appName: string; windowTitle: string; supported: boolean; bundlePath?: string | null }[],
): PickerGrid {
  return core("grid.apps", {
    state: s,
    windows: windows.map((w) => ({
      windowId: w.windowId,
      appId: w.appId,
      appName: w.appName,
      windowTitle: w.windowTitle,
      supported: w.supported,
      bundlePath: w.bundlePath ?? undefined,
    })),
  });
}

/** The Open windows tab as tiles: this machine's windows matching `query`. */
export function windowGrid(
  windows: { windowId: number; appId: string; appName: string; windowTitle: string; supported: boolean; bundlePath?: string | null }[],
  query: string,
  selected: string | null,
): PickerGrid {
  return core("grid.windows", {
    windows: windows.map((w) => ({
      windowId: w.windowId,
      appId: w.appId,
      appName: w.appName,
      windowTitle: w.windowTitle,
      supported: w.supported,
      bundlePath: w.bundlePath ?? undefined,
    })),
    query,
    selected,
  });
}

/** The From <Space> tab as tiles: the Space's windows matching `query`
 * (never its screen target). */
export function remoteGrid(
  windows: {
    id: string;
    appName: string;
    title: string;
    visible: boolean;
    appId: string;
    targetEpoch: number;
    widthPx?: number;
    heightPx?: number;
    pid?: number;
  }[],
  query: string,
  selected: string | null,
): PickerGrid {
  return core("grid.remote", {
    windows: windows.map((w) => ({
      id: w.id,
      appName: w.appName,
      title: w.title,
      visible: w.visible,
      appId: w.appId,
      targetEpoch: w.targetEpoch,
      widthPx: w.widthPx,
      heightPx: w.heightPx,
      pid: w.pid,
    })),
    query,
    selected,
  });
}

export type PickerGridTab = "apps" | "windows" | "space";

/** The grid's primary button: "Continue", "Teleport to <Space>" or
 * "Stream to This Mac", live with a choosable tile selected. */
export function gridPrimary(tab: PickerGridTab, spaceName: string, grid: PickerGrid): { label: string; enabled: boolean } {
  return core("grid.primary", { tab, spaceName, grid });
}

/** Arrow keys: the tile `delta` choosable places from `selected`. */
export function gridStep(grid: PickerGrid, selected: string | null, delta: number): string | null {
  return core("grid.step", { grid, selected, delta });
}

/** The picker's tab strip ("From <Space>" names the Space). */
export function gridTabs(spaceName: string): { tab: PickerGridTab; label: string }[] {
  return core("grid.tabs", { spaceName });
}

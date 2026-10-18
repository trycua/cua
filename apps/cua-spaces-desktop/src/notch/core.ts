// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The app core's notch functions, as the cua-spaces-ffi Node bindings export
// them (uniffi: camelCase names and fields, flat enums as numbers in
// declaration order, tagged enums as `{ tag, inner }`, u64/i64 as bigint),
// and the conversion of their results into the notch protocol's plain
// shapes. Only the members the notch uses are declared, so the bindings
// module itself satisfies `NotchCore`.

import type { Activity, ActivityKind, ButtonId, NotchLayout, NotchMotion, NotchView, Phase, Radii, Rect, ScreenFacts, Tile, TileStatus } from "./protocol";

/** A uniffi tagged-enum value. */
export interface Tagged {
  tag: string;
  inner?: Record<string, unknown>;
}

/** `AppNotchState`: opaque here except what the notch reads. */
export interface CoreNotchState {
  open: boolean;
  query: string;
  drag: { phase: unknown };
  hotspot: boolean;
  transfer?: unknown;
  keyvault?: string;
  signedIn: string[];
  hidden: boolean;
}

/** `AppDragTriggerState`: opaque here except what the notch reads. */
export interface CoreTriggerState {
  kind: unknown;
  phase: unknown;
}

export interface NotchCore {
  appNotchInitial(): CoreNotchState;
  appNotchReduce(state: CoreNotchState, event: Tagged): { state: CoreNotchState; effects: Tagged[] };
  appNotchView(state: CoreNotchState, spaces: readonly unknown[]): unknown;
  appNotchLayout(screen: ScreenFacts, prompt: boolean): NotchLayout;
  appNotchMotion(): NotchMotion;
  appNotchRadii(): Radii[];
  appNotchTileAt(layout: NotchLayout, tiles: readonly unknown[], row: boolean, x: number, y: number): string | undefined;
  appNotchEstimatedProgress(elapsedMs: bigint, estimateMs: number): number;
  appOsIconSvg(id: string): string | undefined;
  appOsIconSystemSymbol(id: string): string | undefined;
  appDragTriggerInitial(): CoreTriggerState;
  appDragTriggerApply(
    state: CoreTriggerState,
    event: Tagged,
    displays: readonly unknown[],
  ): { state: CoreTriggerState; overlay: Tagged[]; tickAtMs?: bigint | number };
  appDragDisplays(screens: ScreenFacts[]): unknown[];
}

// Flat enums in declaration order (the bindings' numbers).
const PHASES: readonly Phase[] = ["closed", "tiles", "prompt"];
const ACTIVITY_KINDS: readonly ActivityKind[] = ["transfer", "remoteAccess", "hotspot", "provisioning", "deleting", "keyvault"];
const BUTTON_IDS: readonly ButtonId[] = ["list", "settings"];
const TILE_STATUSES: readonly TileStatus[] = ["local", "running", "approval", "suspended", "provisioning", "deleting"];
const OVERLAY_PHASES = ["idle", "prompt", "selector"] as const;
const TRIGGER_PHASES = ["hidden", "prompt", "expanded"] as const;
const DRAG_KINDS = ["pending", "move", "resize"] as const;

/** A flat enum value (a number, or a case name in any case) as its name. */
export function enumName<T extends string>(names: readonly T[], value: unknown): T {
  if (typeof value === "number" && names[value] !== undefined) return names[value];
  if (typeof value === "string") {
    const hit = names.find((n) => n.toLowerCase() === value.toLowerCase());
    if (hit) return hit;
  }
  throw new Error(`unknown enum value ${String(value)} (expected one of ${names.join(", ")})`);
}

export const overlayPhase = (v: unknown) => enumName(OVERLAY_PHASES, v);
export const triggerPhase = (v: unknown) => enumName(TRIGGER_PHASES, v);
export const dragKind = (v: unknown) => enumName(DRAG_KINDS, v);

const num = (v: unknown): number => (typeof v === "bigint" ? Number(v) : (v as number));
const opt = <T>(v: T | null | undefined): T | undefined => (v === null ? undefined : v);
const optNum = (v: unknown): number | undefined => (v === null || v === undefined ? undefined : num(v));

type Obj = Record<string, any>;

function rect(r: Obj): Rect {
  return { x: r.x, y: r.y, width: r.width, height: r.height };
}

function tile(t: Obj): Tile {
  return {
    id: t.id,
    name: t.name,
    status: enumName(TILE_STATUSES, t.status),
    dim: t.dim,
    dropTarget: t.dropTarget,
    targeted: t.targeted,
    symbol: t.symbol,
    label: t.label,
    location: t.location,
    progress: optNum(t.progress),
    progressLabel: opt(t.progressLabel),
    signedIn: t.signedIn,
  };
}

function activity(a: Obj): Activity {
  return {
    kind: enumName(ACTIVITY_KINDS, a.kind),
    label: a.label,
    symbol: opt(a.symbol),
    permille: optNum(a.permille),
    startedAt: optNum(a.startedAt),
    estimateMs: num(a.estimateMs),
  };
}

/** The core's `AppNotchView` in the protocol's shape. */
export function wireView(value: unknown): NotchView {
  const v = value as Obj;
  const h = v.header as Obj | undefined | null;
  return {
    phase: enumName(PHASES, v.phase),
    tiles: (v.tiles as Obj[]).map(tile),
    dropMode: v.dropMode,
    prompt: opt(v.prompt),
    label: v.label,
    countLabel: v.countLabel,
    tab: { count: v.tab.count, word: v.tab.word },
    header: h
      ? {
          query: h.query,
          placeholder: h.placeholder,
          searchLabel: h.searchLabel,
          matchCount: optNum(h.matchCount),
          buttons: (h.buttons as Obj[]).map((b) => ({ id: enumName(BUTTON_IDS, b.id), symbol: b.symbol, label: b.label, help: b.help })),
        }
      : undefined,
    empty: opt(v.empty),
    activity: v.activity ? activity(v.activity) : undefined,
    hidden: v.hidden,
    showTab: v.showTab,
    hoverCue: v.hoverCue,
    permission: v.permission ? { text: v.permission.text, action: v.permission.action, pane: v.permission.pane } : undefined,
    access: v.access ? { text: v.access.text, dismiss: v.access.dismiss } : undefined,
  };
}

/** The core's `AppNotchLayout` in the protocol's shape. */
export function wireLayout(value: unknown): NotchLayout {
  const l = value as Obj;
  return {
    hasNotch: l.hasNotch,
    notch: rect(l.notch),
    closedFrame: rect(l.closedFrame),
    openFrame: rect(l.openFrame),
    promptFrame: rect(l.promptFrame),
    tabFrame: rect(l.tabFrame),
    tabInsetNotch: l.tabInsetNotch,
    tabInsetOuter: l.tabInsetOuter,
    stageFrame: rect(l.stageFrame),
    notchStyle: l.notchStyle,
  };
}

/** The core's `AppNotchMotion` in the protocol's shape. */
export function wireMotion(value: unknown): NotchMotion {
  const m = value as Obj;
  const keys: (keyof NotchMotion)[] = [
    "hoverDwellMs",
    "closeDelayMs",
    "openResponse",
    "openDamping",
    "closeResponse",
    "closeDamping",
    "reducedDuration",
    "hoverResponse",
    "hoverDamping",
    "hoverScale",
    "hoverScaleY",
    "contentDelayMs",
    "contentIn",
    "contentOut",
    "contentScale",
  ];
  return Object.fromEntries(keys.map((k) => [k, num(m[k])])) as unknown as NotchMotion;
}

/** Whether the open panel shows a line above the tiles (the layout's row). */
export function needsRow(v: NotchView): boolean {
  return v.prompt !== undefined || v.permission !== undefined || v.access !== undefined;
}

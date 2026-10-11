// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The notch protocol between this app and "Cua Spaces Notch.app" (the Swift
// helper, libs/spaces-notch-swift): one JSON object per line on the helper's
// stdin (from here) and stdout (to here), each with a `type`. Pure, so it is
// tested without Electron; the shapes match the Swift side's `NotchData`,
// `HostMessage` and `HelperMessage` (NotchProtocol.swift documents each).
//
// Why stdio and not a socket: the pipe is private to the two processes (no
// path, no permissions, nothing else can connect), and the helper exits
// when its stdin closes, so it never outlives this app, crash included.

/** Bumped on any incompatible change; the helper refuses another. */
export const NOTCH_PROTOCOL_VERSION = 1;
/** The helper's exit status for a version mismatch (never restarted). */
export const MISMATCH_EXIT = 3;
/** The longest line read from the helper. */
export const MAX_LINE = 1024 * 1024;

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type Phase = "closed" | "tiles" | "prompt";
export type ActivityKind = "transfer" | "remoteAccess" | "hotspot" | "provisioning" | "deleting" | "keyvault";
export type ButtonId = "list" | "settings";
export type TileStatus = "local" | "running" | "approval" | "suspended" | "provisioning" | "deleting";

export interface Tile {
  id: string;
  name: string;
  status: TileStatus;
  dim: boolean;
  dropTarget: boolean;
  targeted: boolean;
  symbol: string;
  label: string;
  location: string;
  progress?: number;
  progressLabel?: string;
  signedIn: boolean;
}

export interface Activity {
  kind: ActivityKind;
  label: string;
  symbol?: string;
  permille?: number;
  startedAt?: number;
  estimateMs: number;
}

export interface NotchView {
  phase: Phase;
  tiles: Tile[];
  dropMode: boolean;
  prompt?: string;
  label: string;
  countLabel: string;
  tab: { count: string; word: string };
  header?: {
    query: string;
    placeholder: string;
    searchLabel: string;
    matchCount?: number;
    buttons: { id: ButtonId; symbol: string; label: string; help: string }[];
  };
  empty?: string;
  activity?: Activity;
  hidden: boolean;
  showTab: boolean;
  hoverCue: boolean;
  permission?: { text: string; action: string; pane: string };
  access?: { text: string; dismiss: string };
}

export interface NotchLayout {
  hasNotch: boolean;
  notch: Rect;
  closedFrame: Rect;
  openFrame: Rect;
  promptFrame: Rect;
  tabFrame: Rect;
  tabInsetNotch: number;
  tabInsetOuter: number;
  stageFrame: Rect;
  notchStyle: boolean;
}

export interface NotchMotion {
  hoverDwellMs: number;
  closeDelayMs: number;
  openResponse: number;
  openDamping: number;
  closeResponse: number;
  closeDamping: number;
  reducedDuration: number;
  hoverResponse: number;
  hoverDamping: number;
  hoverScale: number;
  hoverScaleY: number;
  contentDelayMs: number;
  contentIn: number;
  contentOut: number;
  contentScale: number;
}

export interface Radii {
  top: number;
  bottom: number;
}

export interface ScreenFacts {
  frame: Rect;
  visibleFrame: Rect;
  safeAreaTop: number;
  auxLeftWidth?: number;
  auxRightWidth?: number;
}

export interface OsIcon {
  symbol?: string;
  svg?: string;
}

/** Input from the panel, for the core's reducer. */
export type NotchInput =
  | { kind: "hoverEnter" | "hoverExit" | "click" | "dismiss" | "escape" }
  | { kind: "dropTargeted"; targeted: boolean }
  | { kind: "search"; query: string };

export interface StateMessage {
  type: "state";
  view: NotchView;
  query: string;
  layout?: NotchLayout;
  shown: boolean;
  dragging: boolean;
  icons: Record<string, OsIcon>;
  highlight?: string;
}

/** This app to the helper. */
export type HostMessage =
  | { type: "hello"; v: number; motion: NotchMotion; radii: { closed: Radii; open: Radii } }
  | StateMessage
  | { type: "thumbnail"; id: string; image?: string }
  | { type: "ghost"; image?: string }
  | { type: "quit" };

export type HelperAction =
  | { action: "openSpace"; spaceId: string }
  | { action: "openMain" | "openSettings" | "openAccess" | "dismissAccess" }
  | { action: "openPermissionSettings"; pane: string }
  | { action: "drop"; spaceId: string; paths: string[] };

/** The helper to this app. */
export type HelperMessage =
  | { type: "hello"; v: number; pid: number }
  | { type: "screens"; notch?: ScreenFacts; primary?: ScreenFacts }
  | { type: "event"; event: NotchInput }
  | ({ type: "action" } & HelperAction)
  | { type: "stage"; open: boolean }
  | { type: "error"; message: string };

/** One message as a line. */
export function encodeMessage(message: HostMessage): string {
  return `${JSON.stringify(message)}\n`;
}

/** Splits the helper's stdout into lines; a line over `MAX_LINE` is dropped whole. */
export class LineSplitter {
  private buffer = "";
  private skipping = false;

  push(chunk: string): string[] {
    const lines: string[] = [];
    let rest = chunk;
    for (let nl = rest.indexOf("\n"); nl >= 0; nl = rest.indexOf("\n")) {
      if (this.skipping) this.skipping = false;
      else {
        const line = this.buffer + rest.slice(0, nl);
        if (line.length > 0) lines.push(line);
      }
      this.buffer = "";
      rest = rest.slice(nl + 1);
    }
    if (!this.skipping) this.buffer += rest;
    if (this.buffer.length > MAX_LINE) {
      this.buffer = "";
      this.skipping = true;
    }
    return lines;
  }
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isStr = (v: unknown): v is string => typeof v === "string";

function isRect(v: unknown): v is Rect {
  return isObject(v) && isNum(v.x) && isNum(v.y) && isNum(v.width) && isNum(v.height);
}

function isScreen(v: unknown): v is ScreenFacts {
  return (
    isObject(v) &&
    isRect(v.frame) &&
    isRect(v.visibleFrame) &&
    isNum(v.safeAreaTop) &&
    (v.auxLeftWidth === undefined || isNum(v.auxLeftWidth)) &&
    (v.auxRightWidth === undefined || isNum(v.auxRightWidth))
  );
}

function parseInput(v: unknown): NotchInput | null {
  if (!isObject(v)) return null;
  switch (v.kind) {
    case "hoverEnter":
    case "hoverExit":
    case "click":
    case "dismiss":
    case "escape":
      return { kind: v.kind };
    case "dropTargeted":
      return typeof v.targeted === "boolean" ? { kind: "dropTargeted", targeted: v.targeted } : null;
    case "search":
      return isStr(v.query) ? { kind: "search", query: v.query } : null;
    default:
      return null;
  }
}

function parseAction(m: Record<string, unknown>): HelperMessage | null {
  switch (m.action) {
    case "openSpace":
      return isStr(m.spaceId) ? { type: "action", action: "openSpace", spaceId: m.spaceId } : null;
    case "openMain":
    case "openSettings":
    case "openAccess":
    case "dismissAccess":
      return { type: "action", action: m.action };
    case "openPermissionSettings":
      return isStr(m.pane) ? { type: "action", action: "openPermissionSettings", pane: m.pane } : null;
    case "drop":
      return isStr(m.spaceId) && Array.isArray(m.paths) && m.paths.every(isStr)
        ? { type: "action", action: "drop", spaceId: m.spaceId, paths: m.paths }
        : null;
    default:
      return null;
  }
}

/** One line from the helper, checked; null for anything malformed or unknown. */
export function parseHelperMessage(line: string): HelperMessage | null {
  let m: unknown;
  try {
    m = JSON.parse(line);
  } catch {
    return null;
  }
  if (!isObject(m)) return null;
  switch (m.type) {
    case "hello":
      return isNum(m.v) && isNum(m.pid) ? { type: "hello", v: m.v, pid: m.pid } : null;
    case "screens": {
      if (m.notch !== undefined && !isScreen(m.notch)) return null;
      if (m.primary !== undefined && !isScreen(m.primary)) return null;
      return { type: "screens", notch: m.notch, primary: m.primary };
    }
    case "event": {
      const event = parseInput(m.event);
      return event ? { type: "event", event } : null;
    }
    case "action":
      return parseAction(m);
    case "stage":
      return typeof m.open === "boolean" ? { type: "stage", open: m.open } : null;
    case "error":
      return isStr(m.message) ? { type: "error", message: m.message } : null;
    default:
      return null;
  }
}

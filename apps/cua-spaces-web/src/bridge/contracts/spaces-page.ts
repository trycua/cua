// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The new Spaces page's decisions, as the app core makes them (CUA-2204).
 * Hand-written mirrors (camelCase) of `libs/cua/crates/cua-spaces-app-core/src/`
 * `space_menu.rs`, `bulk_confirm.rs`, `tips.rs`, `account.rs` and the Pro
 * banner in `upsell.rs`.
 */

// ---- A Space's "…" menu (`spaces.menu`) ----

export type SpaceMenuKind = "mine" | "withMe" | "team";
export type SpaceMenuRole = "owner" | "editor" | "viewer";
export type SpaceMenuPower = "running" | "off" | "busy";

/** What the menu reads (`space_menu::SpaceMenuInput`); every field defaults. */
export interface SpaceMenuInput {
  kind?: SpaceMenuKind;
  role?: SpaceMenuRole;
  /** A team's Space: you made it (Delete Space in place of Leave). */
  maker?: boolean;
  power?: SpaceMenuPower;
  /** Yours and shared with at least one person (Manage sharing…). */
  shared?: boolean;
  canPower?: boolean;
  canTeleport?: boolean;
  canShare?: boolean;
  canRename?: boolean;
  canDelete?: boolean;
  canLeave?: boolean;
}

export type SpaceMenuItemId = "open" | "power" | "share" | "manageSharing" | "teleport" | "rename" | "delete" | "leave";

export interface SpaceMenuItem {
  id: SpaceMenuItemId;
  label: string;
  enabled: boolean;
  /** Drawn in red (Delete Space only). */
  destructive: boolean;
  separatorBefore: boolean;
}

// ---- Bulk delete and leave (`spaces.bulkConfirm`, `bulkProgress`, `bulkDone`) ----

export interface BulkPerson {
  who: string;
  name?: string | null;
}

/** One selected Space (`bulk_confirm::BulkItem`). */
export interface BulkItem {
  id: string;
  name: string;
  os: string;
  running?: boolean;
  /** Its size on disk (yours only; null: not known). */
  bytes?: number | null;
  /** Yours (deleted); otherwise left. */
  mine: boolean;
  /** "This Mac", "Sam's Mac mini", "QA team". */
  place: string;
  /** A team's Space. */
  team?: boolean;
  /** Who else has it (yours): they lose access. */
  people?: BulkPerson[];
}

export interface BulkRow {
  id: string;
  name: string;
  os: string;
  /** "Running". */
  status: string | null;
  /** Who has it, for the small stack. */
  people: string[];
  /** The size of yours ("14 GB") or the computer of theirs; null: "—". */
  detail: string | null;
}

export interface BulkGroup {
  /** "Delete", "Leave" or a computer's name; null: a single group. */
  title: string | null;
  rows: BulkRow[];
}

export interface BulkConfirmView {
  title: string;
  body: string;
  groups: BulkGroup[];
  /** "Frees". */
  freesLabel: string;
  /** "29 GB" (null: nothing of yours with a known size). */
  frees: string | null;
  cancel: string;
  button: string;
  /** The button is red. */
  destructive: boolean;
  /** For `spaces.deleteMany`. */
  deleteIds: string[];
  /** For `spaces.leave`. */
  leaveIds: string[];
}

/** One Space's outcome (`spaces.deleteMany` / `spaces.leave`). */
export interface BulkResult {
  id: string;
  ok: boolean;
  error?: string | null;
}

export interface BulkDone {
  /** "Deleted 3 Spaces. 29 GB freed." */
  text: string;
  tone: "ok" | "partial" | "failed";
}

// ---- Tips (`tips.view`, `tips.reduce`) ----

export type TipId = "try-agent" | "power" | "bulk-select" | "share" | "on-auto";
export type TipPlace = "spaces" | "space" | "sidebar";

/** Kept in the app settings (`AppSettings.tips`). */
export interface TipsState {
  off: boolean;
  done: string[];
}

/** What the tips read (`tips::TipFacts`); every field defaults. */
export interface TipFacts {
  /** The newest running Space's name (the Spaces page). */
  newestRunning?: string | null;
  /** On a running Space's page: its name. */
  spacePage?: string | null;
  /** Agents with Cua set up (setup ids: hermes, claude-code, codex, cursor, ...). */
  agentsConnected?: string[];
  /** Agents found on this computer (setup ids). */
  agentsInstalled?: string[];
  hasStopped?: boolean;
  deletableStopped?: number;
  diskNearlyFull?: boolean;
  secondComputerOnline?: boolean;
  sharedAny?: boolean;
  /** A page shows a tip now (the sidebar's waits). */
  pageTipShown?: boolean;
}

export type TipActionKind = "none" | "copy" | "selectStopped" | "share" | "copyCommand";

export interface TipAction {
  kind: TipActionKind;
  /** "Copy", "Select stopped", "Share…"; empty: no button. */
  label: string;
  /** What a copy puts on the clipboard. */
  value: string | null;
  /** "Copied". */
  doneLabel: string | null;
}

export interface Tip {
  id: TipId;
  place: TipPlace;
  text: string;
  /** Read-only text with Copy (the request, the command). */
  code: string | null;
  /** The agent the line names (setup id; null: "your agent"). */
  agent: string | null;
  action: TipAction;
  /** "Next tip" (null in the sidebar). */
  next: string | null;
  /** "Don't show tips" (null in the sidebar). */
  dontShow: string | null;
  /** The ×'s accessibility label. */
  close: string;
  /** Opens the request in the named agent (Cursor only, next to Copy). */
  openIn: { label: string; url: string } | null;
}

export type TipsEvent =
  | { type: "next"; id: string }
  | { type: "close"; id: string }
  | { type: "dont-show" }
  | { type: "powered" }
  | { type: "bulk-deleted" }
  | { type: "shared" }
  | { type: "show-tips"; on: boolean }
  | { type: "reset" };

/** The core's `AppSettings` fields the new Spaces page reads; the rest pass through. */
export interface CoreAppSettings {
  tips: TipsState;
  [field: string]: unknown;
}

// ---- The account area (`upsell.proBanner`, `account.menu`) ----

export interface ProBannerInput {
  /** The relay's plan word (`free`, `pro`, `team`; empty: not known). */
  plan: string;
}

export interface ProBanner {
  visible: boolean;
  /** "Run more Spaces at once with Pro". */
  text: string;
  /** "Upgrade". */
  button: string;
}

export interface AccountMenuInput {
  plan: string;
  signedIn: boolean;
  /** `SessionSnapshot.fleet.authMode`: only `user` signs out. */
  authMode: string;
}

export interface AccountMenuItem {
  id: "settings" | "plan" | "signOut";
  label: string;
  /** The plan word ("Free", "Pro", "Teams"). */
  value: string | null;
  separatorBefore: boolean;
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Share sheet, as the app core models it. Hand-written mirrors of
 * `libs/cua/crates/cua-spaces-app-core/src/share.rs` (camelCase).
 */

export type ShareRole = "viewer" | "editor";

/** One person a Space is shared with, as the host reports it. */
export interface ShareEntry {
  who: string;
  role: string;
  connected?: boolean;
}

/** Everything the sheet reads (`share::ShareInput`). */
export interface ShareInput {
  spaceId: string;
  spaceName: string;
  shares: ShareEntry[];
  signedIn: boolean;
  /** A host, or a driver with `relay_attach`. */
  shareable: boolean;
}

/** The command the host runs next (`share::ShareRequest`). */
export type ShareRequest =
  | { kind: "share"; space: string; who: string; role: string }
  | { kind: "unshare"; space: string; who: string };

/** The sheet's state (`share::ShareSheetState`). */
export interface ShareSheetState {
  who: string;
  role: string;
  busy: boolean;
  error: string | null;
  request: ShareRequest | null;
}

/** An input to the sheet (`share::ShareSheetAction`). */
export type ShareSheetAction =
  | { type: "set-who"; who: string }
  | { type: "set-role"; role: string }
  | { type: "submit" }
  | { type: "change-role"; who: string; role: string }
  | { type: "remove"; who: string }
  | { type: "done" }
  | { type: "failed"; error: string };

export interface ShareRowView {
  who: string;
  role: string;
  connected: boolean;
}

/** The sheet as drawn (`share::ShareSheetView`); every word is the core's. */
export interface ShareSheetView {
  title: string;
  rows: ShareRowView[];
  emptyText: string;
  who: string;
  whoPlaceholder: string;
  hint: string | null;
  role: string;
  roles: { id: string; label: string }[];
  shareLabel: string;
  canShare: boolean;
  removeLabel: string;
  doneLabel: string;
  disabledReason: string | null;
  busy: boolean;
  error: string | null;
  request: ShareRequest | null;
}

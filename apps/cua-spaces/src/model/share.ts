// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Share sheet: who a Space is shared with (viewer or editor) and the
 * command to run. Every word and decision is the app core's (`share.*`,
 * the same the SwiftUI app binds); this module only types it.
 */

import { core } from '../core';

export interface ShareEntryInput {
  who: string;
  role: 'viewer' | 'editor' | string;
  connected?: boolean;
}

/** What the sheet reads: the Space and the `space_shares` result. */
export interface ShareInput {
  spaceId: string;
  spaceName: string;
  shares: ShareEntryInput[];
  signedIn: boolean;
  shareable: boolean;
}

export type ShareRequest =
  | { kind: 'share'; space: string; who: string; role: string }
  | { kind: 'unshare'; space: string; who: string };

export interface ShareSheetState {
  who: string;
  role: string;
  busy: boolean;
  error: string | null;
  request: ShareRequest | null;
}

export type ShareSheetAction =
  | { type: 'set-who'; who: string }
  | { type: 'set-role'; role: string }
  | { type: 'submit' }
  | { type: 'change-role'; who: string; role: string }
  | { type: 'remove'; who: string }
  | { type: 'done' }
  | { type: 'failed'; error: string };

export interface RoleOption {
  id: string;
  label: string;
}

export interface ShareRowView {
  who: string;
  role: string;
  connected: boolean;
}

export interface ShareSheetView {
  title: string;
  rows: ShareRowView[];
  emptyText: string;
  whoPlaceholder: string;
  who: string;
  role: string;
  roles: RoleOption[];
  canShare: boolean;
  shareLabel: string;
  removeLabel: string;
  doneLabel: string;
  disabledReason: string | null;
  hint: string | null;
  busy: boolean;
  error: string | null;
  request: ShareRequest | null;
}

export function shareInitial(): ShareSheetState {
  return core('share.initial');
}

export function reduceShare(
  input: ShareInput,
  state: ShareSheetState,
  action: ShareSheetAction
): ShareSheetState {
  return core('share.reduce', { input, state, action });
}

export function shareView(input: ShareInput, state: ShareSheetState): ShareSheetView {
  return core('share.view', { input, state });
}

/** Why the sheet cannot share and where to go (`share.gate`). */
export interface ShareGate {
  reason: string;
  action: { label: string; url: string };
}

/**
 * The gate for an account: closed with an explanation and a link to Teams
 * when it is known to be in no team that can share. `null` leaves the sheet
 * open: in a team, or not known (the server decides). This only explains;
 * the control plane and the relay refuse an outside share either way.
 */
export function shareGate(hasTeam: boolean | null): ShareGate | null {
  return core('share.gate', { hasTeam });
}

/** A refused share in plain words (`share_outside_team`, `sharing_needs_team`); null for any other error. */
export function shareErrorWords(error: string, who: string): string | null {
  return core('share.errorWords', { error, who });
}

/** Someone removed from a shared computer (`sharing.removals`). */
export interface ShareRemoval {
  who: string;
  machineName: string;
}

/** The owner's one-line note about them (`share.removalNote`). */
export interface ShareRemovalNote {
  text: string;
  action: { label: string; url: string };
  dismiss: string;
}

export function shareRemovalNote(removals: ShareRemoval[]): ShareRemovalNote | null {
  return core('share.removalNote', { removals });
}

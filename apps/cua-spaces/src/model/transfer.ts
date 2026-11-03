// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";

/**
 * Transfer-overlay state (teleport → instant Space view, item B).
 *
 * When a teleport push begins, the Space's desktop window is opened/focused and
 * an overlay with an indeterminate progress bar sits over it for the duration
 * of the transfer. The picker window drives the push and relays lifecycle
 * signals to the viewer window (`space-transfer` events / `viewer_config`);
 * this pure reducer turns those signals into the overlay's render state.
 *
 * active → the indeterminate bar sweeps; "Teleporting {app}…".
 * error  → the transfer failed; a Retry re-runs the push.
 * (done clears the overlay entirely.)
 */

export type TransferPhase = "active" | "error";

export interface TransferOverlayState {
  phase: TransferPhase;
  /** Display name of the app being teleported. */
  appName: string;
  /** Failure reason, present only in the error phase. */
  message?: string;
  /**
   * Bytes uploaded so far, once the CLI reports real upload progress. When
   * both `sentBytes` and `totalBytes` are present the overlay shows a
   * DETERMINATE bar and a "{x} MB / {y} MB" label; while they're absent (the
   * brief pre-upload/export phase) the bar is indeterminate.
   */
  sentBytes?: number;
  /** Total bytes to upload (the buffered bundle size). */
  totalBytes?: number;
}

/** Lifecycle signal from the shell/picker. */
export interface TransferSignal {
  status: "start" | "progress" | "done" | "error";
  appName?: string;
  message?: string;
  sentBytes?: number;
  totalBytes?: number;
}

/** Advance the overlay; `done` clears it (null). The app core decides. */
export function reduceTransfer(
  state: TransferOverlayState | null,
  signal: TransferSignal,
): TransferOverlayState | null {
  return core("transfer.reduce", { state, signal });
}

/** Overlay heading for a phase. */
export function transferTitle(state: TransferOverlayState): string {
  return core("transfer.title", { state });
}

/** "12.3 MB": MB with one decimal place. */
export function formatMegabytes(bytes: number): string {
  return core("transfer.formatMegabytes", { bytes });
}

/** The determinate fraction in `[0, 1]`, or null while totals are unknown. */
export function transferProgress(state: TransferOverlayState): number | null {
  return core("transfer.progress", { state });
}

/** "{x} MB / {y} MB", or null while totals are unknown. */
export function transferSizeLabel(state: TransferOverlayState): string | null {
  return core("transfer.sizeLabel", { state });
}

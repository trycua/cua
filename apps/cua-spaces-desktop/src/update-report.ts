// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** What a failed update check or download is reported as; set by crash-reporting-main once Sentry runs. */
export interface UpdateFailureReport {
  /** `code-ENOTFOUND`, `http-404`, `other`: groups one outage into one issue. */
  kind: string;
  /** The error as the user would read it (describeUpdateError). */
  reason: string;
  error: Error;
}

let sink: ((report: UpdateFailureReport) => void) | null = null;

/** Where failures go (null: nowhere, as in a development run or with crash reports off). */
export function setUpdateFailureSink(next: ((report: UpdateFailureReport) => void) | null): void {
  sink = next;
}

/**
 * Hands a failed update check or download to the sink as a handled event
 * (the updater state machine has already shown it). Never throws.
 */
export function reportUpdateFailure(error: unknown, reason: string): void {
  try {
    const e = error !== null && typeof error === "object" ? (error as { code?: unknown; statusCode?: unknown }) : {};
    const kind =
      typeof e.code === "string" || typeof e.code === "number" ? `code-${e.code}` : typeof e.statusCode === "number" ? `http-${e.statusCode}` : "other";
    sink?.({ kind, reason, error: error instanceof Error ? error : new Error(reason) });
  } catch {
    // Reporting a failure must not become one.
  }
}

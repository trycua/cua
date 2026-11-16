// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Whether an error is the connection's (a message is queued and tried again) or the request's (it counts against the message). */

const WORDS =
  /network request failed|failed to fetch|network error|network connection|timed out|timeout|offline|no route|econn|enotfound|not connected to the internet|load failed|the internet connection|unreachable|fetch failed|socket/i;

/** Connect error codes that mean the connection: `Unavailable` (14), `DeadlineExceeded` (4), `Aborted` (10). */
const CONNECT_CODES = new Set([14, 4, 10]);

export function isNetworkError(e: unknown): boolean {
  if (!e || typeof e !== "object") return typeof e === "string" && WORDS.test(e);
  const err = e as { name?: string; code?: unknown; message?: string; status?: unknown };
  if (err.name === "TypeError") return true;
  if (err.name === "ConnectError" && typeof err.code === "number" && CONNECT_CODES.has(err.code)) return true;
  if (err.name === "RelayError" && (err.status === 0 || err.status === 502 || err.status === 503 || err.status === 504)) return true;
  return WORDS.test(err.message ?? "");
}

export const errorText = (e: unknown): string => (e instanceof Error ? e.message : typeof e === "string" ? e : "the computer refused it");

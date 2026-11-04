// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A failed update check or download is handled (Settings → About shows it),
// so it is logged with its reason, not left to Sentry's global handlers as a
// crash. electron-updater's errors can have an empty message (the NSIS
// download failure on Windows), so the reason is built from the error's name,
// code, HTTP status and the URL's host (never the path or query).

interface UpdateErrorLike {
  name?: unknown;
  message?: unknown;
  code?: unknown;
  statusCode?: unknown;
  url?: unknown;
}

const hostOf = (value: unknown): string | null => {
  if (typeof value !== "string") return null;
  const found = /https?:\/\/[^\s"'<>)]+/.exec(value)?.[0];
  if (!found) return null;
  try {
    return new URL(found).host;
  } catch {
    return null;
  }
};

/** A readable reason for a handled update failure; never empty. */
export function describeUpdateError(error: unknown): string {
  const e: UpdateErrorLike = error !== null && typeof error === "object" ? (error as UpdateErrorLike) : { message: String(error ?? "") };
  const message = typeof e.message === "string" ? e.message.trim() : "";
  const parts: string[] = [];
  if (typeof e.code === "string" || typeof e.code === "number") parts.push(`code ${e.code}`);
  if (typeof e.statusCode === "number") parts.push(`HTTP ${e.statusCode}`);
  const host = hostOf(e.url) ?? hostOf(message);
  if (host) parts.push(`host ${host}`);
  const name = typeof e.name === "string" && e.name ? e.name : "Error";
  const head = message || `${name} with no message`;
  return parts.length ? `${head} (${parts.join(", ")})` : head;
}

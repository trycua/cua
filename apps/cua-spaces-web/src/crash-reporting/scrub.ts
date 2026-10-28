// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What may leave the page in a crash report (CUA-2343). The same rules as the
 * Electron shell's (apps/cua-spaces-desktop/src/crash-reporting.ts): messages
 * are cleared (they can carry Space names, file names and emails), and
 * anything else is scrubbed of home folders and emails, or dropped. Pure, so
 * it runs under test; `index.ts` applies it in `beforeSend`.
 */

const EMAIL = /[A-Za-z0-9._%+'-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;
const HOME_UNIX = /(?:\/Users|\/home)\/[^/\\\s"'`:;,)\]}]+/g;
const HOME_WIN = /[A-Za-z]:[\\/]+Users[\\/]+[^\\/"'`:;,)\]}]+/gi;

/** Home folders (`/Users/x`, `/home/x`, `C:\Users\x`) become `~`, emails `[email]`. */
export function scrubString(value: string): string {
  return value.replace(HOME_WIN, "~").replace(HOME_UNIX, "~").replace(EMAIL, "[email]");
}

/** A URL without its query string and fragment. */
export function stripUrl(url: string): string {
  return url.replace(/[?#].*$/, "");
}

function scrubDeep(value: unknown, depth = 0): unknown {
  if (typeof value === "string") return scrubString(/^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? stripUrl(value) : value);
  if (depth > 12 || value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map((v) => scrubDeep(v, depth + 1));
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(value)) out[k] = scrubDeep(v, depth + 1);
  return out;
}

/** The parts of a Sentry event the scrub touches. */
export interface ScrubbableEvent {
  message?: string;
  logentry?: unknown;
  exception?: { values?: { type?: string; value?: string; module?: string; stacktrace?: { frames?: Record<string, unknown>[] } }[] };
  [key: string]: unknown;
}

function scrubFrame(frame: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...frame };
  for (const k of ["filename", "abs_path"]) if (typeof out[k] === "string") out[k] = scrubString(stripUrl(out[k] as string));
  for (const k of ["module", "function"]) if (typeof out[k] === "string") out[k] = scrubString(out[k] as string);
  for (const k of ["context_line", "pre_context", "post_context", "vars"]) delete out[k];
  return out;
}

/**
 * The event as it may leave the page: exception values, the message and log
 * entry cleared; type, module and frames kept with paths scrubbed; user,
 * breadcrumbs, attachments, request headers and bodies, server and device
 * names dropped; a request URL without query or fragment. Returns a new object.
 */
export function scrubEvent<T extends ScrubbableEvent>(event: T): T {
  const e: ScrubbableEvent = { ...event };
  for (const k of ["user", "breadcrumbs", "logentry", "message", "attachments", "threads"]) delete e[k];
  e.server_name = "";
  if (e.exception?.values) {
    e.exception = {
      ...e.exception,
      values: e.exception.values.map((v) => ({
        ...v,
        value: "",
        ...(v.stacktrace?.frames ? { stacktrace: { ...v.stacktrace, frames: v.stacktrace.frames.map(scrubFrame) } } : {}),
      })),
    };
  }
  if (e.request && typeof e.request === "object") {
    const url = (e.request as { url?: unknown }).url;
    e.request = typeof url === "string" ? { url: scrubString(stripUrl(url)) } : {};
  }
  for (const key of ["tags", "extra", "contexts", "debug_meta", "modules", "sdk", "fingerprint", "transaction", "culprit"]) {
    if (e[key] !== undefined) e[key] = scrubDeep(e[key]);
  }
  const contexts = e.contexts as Record<string, Record<string, unknown>> | undefined;
  if (contexts) {
    if (contexts.device) {
      delete contexts.device.name;
      delete contexts.device.hostname;
    }
    delete contexts.culture;
  }
  return e as T;
}

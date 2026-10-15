// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `cua://add/<code>`: the add-machine link (https://cua.ai/add/<code>)
// opened in this app. It brings the app forward on Machines with Join with a
// code filled in. It never joins on its own: the page asks first.
import { normalizeCode } from "./bridge/join-with-code";

export const DEEP_LINK_SCHEME = "cua";

/**
 * The app's route for a `cua://add/<code>[?relay=<url>]` link (also
 * `cua://add?code=<code>`), or null for any other URL or a malformed code.
 */
export function routeForDeepLink(raw: string): string | null {
  let url: URL;
  try {
    url = new URL(raw.trim());
  } catch {
    return null;
  }
  if (url.protocol !== `${DEEP_LINK_SCHEME}:` || url.hostname.toLowerCase() !== "add") return null;
  const fromPath = decodeURIComponent(url.pathname.replace(/^\/+/, "").split("/")[0] ?? "");
  const code = normalizeCode(fromPath || url.searchParams.get("code") || "");
  if (!code) return null;
  const relay = url.searchParams.get("relay")?.trim();
  const query = new URLSearchParams({ join: code });
  if (relay && /^https?:\/\//i.test(relay)) query.set("relay", relay);
  return `/machines?${query.toString()}`;
}

/** The first `cua://` argument of a command line (Windows and Linux start a second copy with the link as an argument). */
export function deepLinkInArgv(argv: readonly string[]): string | null {
  return argv.find((a) => a.toLowerCase().startsWith(`${DEEP_LINK_SCHEME}://`)) ?? null;
}

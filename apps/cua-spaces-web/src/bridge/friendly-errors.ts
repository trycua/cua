// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Plain words for the host's raw failures. The `cua` command and the relay
 * word errors for logs ("cua: unauthenticated: host setup failed at
 * register: relay: ..."); a person gets one short line. The raw text goes to
 * the console log (the app's log file), never away.
 */

/** Internal prefixes the command and relay put in front of the part a person can use. */
const PREFIX = /^(?:(?:cua|relay|error|unauthenticated|not[ _]found|host setup failed at [\w-]+)\s*:\s*)+/i;

/** `text` without internal prefixes ("cua: relay: ..." becomes "..."), first letter capitalised. */
export function stripInternalPrefixes(text: string): string {
  const s = text.trim().replace(PREFIX, "").trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : "";
}

function rawText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** What "Join with a code" says for a failure. */
export function plainJoinError(e: unknown): string {
  const raw = rawText(e);
  console.warn("[Cua Spaces] join with a code failed:", raw);
  if (/pairing code|unauthenticated|not valid|expired|revoked|already used|invalid code/i.test(raw)) {
    return "That code didn't work. It may be used, expired or mistyped. Make a new one in Add a computer.";
  }
  if (/timed out/i.test(raw)) return "This took too long. Check the internet connection and try again.";
  if (/could not start|no bundled cua/i.test(raw)) return "Couldn't set this computer up. Update Cua Spaces and try again.";
  if (/network|connect|dns|unreachable|offline/i.test(raw)) return "Couldn't reach Cua. Check the internet connection and try again.";
  return stripInternalPrefixes(raw) || "Couldn't join with that code.";
}

/** What "Add a machine" says when it can't make a link. */
export function plainPairingError(e: unknown): string {
  const raw = rawText(e);
  console.warn("[Cua Spaces] making a pairing link failed:", raw);
  if (/no such device|not enrolled|not registered|unknown device/i.test(raw)) {
    return "This computer isn't set up for access yet. Set it up first, then add computers.";
  }
  if (/unauthenticated|unauthori[sz]ed|\b40[13]\b/i.test(raw)) return "Sign in again to add a computer.";
  if (/network|connect|dns|unreachable|offline|timed out/i.test(raw)) return "Couldn't reach Cua. Check the internet connection.";
  const rest = stripInternalPrefixes(raw).replace(/\.$/, "");
  return rest ? `Couldn't make a link: ${rest}.` : "Couldn't make a link.";
}

/**
 * The This machine panel's limits line for this system. The core words it
 * for a Mac ("Up to 4 Spaces · 2 macOS VMs"); macOS VMs run on a Mac only,
 * so elsewhere that part is dropped.
 */
export function limitsFor(limits: string, mac: boolean): string {
  return mac ? limits : limits.replace(/\s*·\s*\d+ macOS VMs?\b.*$/, "");
}

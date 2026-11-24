// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Reading a pairing code from what a person typed, pasted or scanned. The
 * same rules as `cua_host::normalize_pairing_code`, so the page accepts
 * what `cua host setup --pair` accepts: 12 Crockford base32 characters in
 * any case, with or without dashes and spaces, `I`/`L` read as `1` and `O`
 * as `0`, or an add-machine link (`https://cua.ai/add/<code>`).
 */

const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN = 12;

/** The code in canonical form (12 upper-case characters), or null when `input` can't be a code. */
export function normalizePairingCode(input: string): string | null {
  let s = input.trim();
  const at = s.indexOf("/add/");
  if (at >= 0) s = s.slice(at + "/add/".length).split(/[?#/]/)[0] ?? "";
  if (s.startsWith("cpc_")) s = s.slice(4);
  let out = "";
  for (const raw of s) {
    const c = raw.toUpperCase();
    if (c === "-" || c === " ") continue;
    const mapped = c === "I" || c === "L" ? "1" : c === "O" ? "0" : c;
    if (mapped.length !== 1 || !ALPHABET.includes(mapped)) return null;
    out += mapped;
  }
  return out.length === CODE_LEN ? out : null;
}

/** `XXXX-XXXX-XXXX`; input that is not a code comes back as typed. */
export function displayPairingCode(input: string): string {
  const c = normalizePairingCode(input);
  return c ? `${c.slice(0, 4)}-${c.slice(4, 8)}-${c.slice(8)}` : input;
}

/** The relay an add-machine link names (`?relay=<url>`), or null. */
export function pairingLinkRelay(input: string): string | null {
  try {
    const v = new URL(input.trim()).searchParams.get("relay")?.trim();
    return v ? v : null;
  } catch {
    return null;
  }
}

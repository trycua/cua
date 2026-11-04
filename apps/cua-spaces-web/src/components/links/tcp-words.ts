// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { linkEnds, type ShareLink } from "@/bridge";

/** Addresses typed as "1.2.3.4/32, 10.0.0.0/8" (commas, spaces or new lines) as a list. */
export function parseAllow(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((x) => x.trim())
    .filter(Boolean);
}

/** The first thing wrong with a typed address list, in plain words; null when it is fine. IPv4 only, with an optional /bits. */
export function allowProblem(list: string[]): string | null {
  for (const item of list) {
    const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?:\/(\d{1,2}))?$/.exec(item);
    if (!m || m.slice(1, 5).some((n) => Number(n) > 255) || (m[5] !== undefined && Number(m[5]) > 32)) {
      return `"${item}" is not an address like 203.0.113.0/24`;
    }
  }
  return null;
}

/** Who may connect: "Anyone", "Only 2 addresses". */
export function whoWords(allow: string[] | undefined): string {
  const n = allow?.length ?? 0;
  if (n === 0) return "Anyone";
  return n === 1 ? "Only 1 address" : `Only ${n} addresses`;
}

/** The port of a `space:NAME:PORT` target. */
export const portOf = (l: ShareLink) => l.target.slice(l.target.lastIndexOf(":") + 1);

/** The Space of a `space:NAME:PORT` target. */
export const spaceOf = (l: ShareLink) => l.target.replace(/^space:/, "").replace(/:\d+$/, "");

/** "Anyone can connect. Ends in 23 h." / "Only 2 addresses can connect. Until stopped." */
export const shareFoot = (l: ShareLink, nowMs?: number) =>
  `${whoWords(l.allow)} can connect. ${l.expiresAtUnix === null ? "Until stopped" : `Ends in ${linkEnds(l, nowMs)}`}.`;

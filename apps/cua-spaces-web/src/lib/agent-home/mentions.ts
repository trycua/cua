// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Where the person said the job runs, in their own words: an @mention of a
 * Space or a machine, "on my Mac", or "connect to <name>". Anything else is
 * left to the agent, which decides.
 */

import type { HomeMentions } from "./abilities";

export interface Named {
  id: string;
  name: string;
}

export interface Place {
  /** Spaces and machines the person can name. */
  spaces: readonly Named[];
  machines: readonly (Named & { current?: boolean })[];
}

const norm = (s: string) => s.trim().toLowerCase();
/** The first word of a name, without a possessive ("Dillon's MacBook" is "dillon"). */
const firstWord = (s: string) => norm(s.split(/\s+/)[0] ?? "").replace(/['\u2019]s$/, "");

/** Names with spaces in them can be mentioned as `@"spare mac"` or by their first word. */
function mentionTokens(text: string): string[] {
  const out: string[] = [];
  for (const m of text.matchAll(/@"([^"]+)"|@([\w.:-]+)/g)) out.push(m[1] ?? m[2] ?? "");
  return out.filter(Boolean);
}

/** The Spaces and machines a request names. */
export function parseMentions(text: string, place: Place): HomeMentions {
  const spaces = new Map<string, Named>();
  const machines = new Map<string, Named>();
  const by = <T extends Named>(list: readonly T[], t: string) =>
    list.find((x) => norm(x.name) === norm(t) || x.id === t) ?? list.find((x) => firstWord(x.name) === norm(t));
  for (const t of mentionTokens(text)) {
    const machine = by(place.machines, t);
    if (machine) machines.set(machine.id, { id: machine.id, name: machine.name });
    else {
      const space = by(place.spaces, t);
      if (space) spaces.set(space.id, { id: space.id, name: space.name });
    }
  }
  // "on my Mac", "on this Mac", "use my laptop": this machine.
  if (/\bon (my|this) (mac|laptop|machine|computer)\b|\buse my (mac|laptop|machine)\b/i.test(text)) {
    const here = place.machines.find((m) => m.current);
    if (here) machines.set(here.id, { id: here.id, name: here.name });
  }
  // "connect to <name>": a machine or Space by that name.
  for (const m of text.matchAll(/\bconnect to (?:@?"([^"]+)"|@?([\w.:-]+))/gi)) {
    const t = m[1] ?? m[2] ?? "";
    const machine = by(place.machines, t);
    if (machine) machines.set(machine.id, { id: machine.id, name: machine.name });
    else {
      const space = by(place.spaces, t);
      if (space) spaces.set(space.id, { id: space.id, name: space.name });
    }
  }
  return { spaces: [...spaces.values()], machines: [...machines.values()] };
}

/** Whether the person said where (so the agent need not decide). */
export function saidWhere(m: HomeMentions): boolean {
  return m.spaces.length + m.machines.length > 0;
}

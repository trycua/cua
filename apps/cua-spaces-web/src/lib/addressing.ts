// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Who a message in a Space goes to when you do not type an @. The page
 * resolves it, so the posted text always names the agent(s) explicitly
 * (the daemon wakes only on an explicit mention):
 *
 * 1. An @ you typed wins. It also becomes the sticky addressee.
 * 2. A mention of a person only is a message to someone else: nothing is added.
 * 3. A Reply to an agent's message addresses that agent.
 * 4. The agents pinned in the composer (the chip).
 * 5. The one agent in the Space, or else the last one addressed.
 *
 * The pinned agents and the last addressed one are kept per Space on this
 * machine (never sent anywhere).
 */

import { create } from "zustand";

import { addLeadingMention, EVERYONE, mentionedHandles } from "./mentions";

/** What the page keeps about a Space's addressee. */
export interface AddressState {
  /** Agents pinned in the composer (`everyone` addresses all of them). */
  pinned: string[];
  /** The agent addressed last (the default when several are here). */
  last: string | null;
  /** The default was dismissed with the chip's X: plain messages until you address someone again. */
  off: boolean;
  /** The "will be mentioned automatically" note was shown once already. */
  told: boolean;
}

export const EMPTY_ADDRESS: AddressState = { pinned: [], last: null, off: false, told: false };

/** Where the addressee came from. */
export type AddressSource = "typed" | "reply" | "pinned" | "only" | "last" | "none";

export interface Addressee {
  handles: string[];
  source: AddressSource;
}

export interface ResolveInput {
  text: string;
  /** Handles of the agents in the Space. */
  agents: readonly string[];
  /** Every handle an agent can answer to, in the Space or able to start in it. */
  agentHandles: ReadonlySet<string>;
  /** Handles of everyone the picker offers (people too). */
  known: ReadonlySet<string>;
  /** `@everyone` is allowed (you may task at least one agent here). */
  canEveryone: boolean;
  state: AddressState;
  /** The agent whose message is being replied to. */
  replyTo?: string | null;
}

/** Who the next message goes to when nothing is typed (the chip shows this). */
export function defaultAddressee(i: Pick<ResolveInput, "agents" | "canEveryone" | "state">): Addressee {
  const here = new Set(i.agents.map((a) => a.toLowerCase()));
  const pinned = i.state.pinned.filter((h) => (h === EVERYONE ? i.canEveryone && here.size > 0 : here.has(h)));
  if (pinned.length) return { handles: pinned, source: "pinned" };
  if (i.state.off) return { handles: [], source: "none" };
  if (i.agents.length === 1) return { handles: [i.agents[0]!.toLowerCase()], source: "only" };
  if (i.state.last && here.has(i.state.last)) return { handles: [i.state.last], source: "last" };
  return { handles: [], source: "none" };
}

/** The text to post, who it goes to, and what to remember afterwards. */
export function resolveAddressing(i: ResolveInput): { text: string; addressee: Addressee; next: AddressState; auto: boolean } {
  const typed = mentionedHandles(i.text, i.known);
  const typedAgents = typed.filter((h) => i.agentHandles.has(h) || (h === EVERYONE && i.canEveryone));
  const remember = (handles: string[]): AddressState => {
    const named = handles.filter((h) => h !== EVERYONE);
    return { ...i.state, pinned: handles, last: named.length ? named[named.length - 1]! : i.state.last, off: false };
  };
  if (typedAgents.length) return { text: i.text, addressee: { handles: typedAgents, source: "typed" }, next: remember(typedAgents), auto: false };
  // Someone else (a person): the pin ends, nothing is added.
  if (typed.length) return { text: i.text, addressee: { handles: [], source: "none" }, next: { ...i.state, pinned: [] }, auto: false };
  const reply = i.replyTo?.toLowerCase();
  if (reply) return { text: addLeadingMention(i.text, reply), addressee: { handles: [reply], source: "reply" }, next: remember([reply]), auto: true };
  const d = defaultAddressee(i);
  if (!d.handles.length) return { text: i.text, addressee: d, next: i.state, auto: false };
  return {
    text: addLeadingMention(i.text, ...d.handles),
    addressee: d,
    // Pinned stays pinned; a default becomes the last addressed.
    next: d.source === "pinned" ? i.state : { ...i.state, last: d.handles[d.handles.length - 1]! },
    auto: true,
  };
}

/* ---- Kept per Space ------------------------------------------------------------------- */

const KEY = "cua.thread.addressing";
const load = (): Record<string, AddressState> => {
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    const all = raw ? (JSON.parse(raw) as Record<string, Partial<AddressState>>) : {};
    return Object.fromEntries(
      Object.entries(all).map(([id, s]) => [
        id,
        {
          pinned: Array.isArray(s.pinned) ? s.pinned.filter((h) => typeof h === "string") : [],
          last: typeof s.last === "string" ? s.last : null,
          off: s.off === true,
          told: s.told === true,
        },
      ]),
    );
  } catch {
    return {};
  }
};

export const useAddressing = create<{ bySpace: Record<string, AddressState> }>(() => ({ bySpace: load() }));
useAddressing.subscribe((s) => {
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(s.bySpace));
  } catch {
    // Not kept past this session.
  }
});

export const setAddress = (spaceId: string, next: AddressState): void => useAddressing.setState((s) => ({ bySpace: { ...s.bySpace, [spaceId]: next } }));

/** Pins `handle` (a Reply), replacing whatever was pinned. */
export function pinAgent(spaceId: string, handle: string): void {
  const cur = useAddressing.getState().bySpace[spaceId] ?? EMPTY_ADDRESS;
  setAddress(spaceId, { ...cur, pinned: [handle.toLowerCase()], last: handle === EVERYONE ? cur.last : handle.toLowerCase(), off: false });
}

/** The chip's X: `handle` leaves the pin; when nothing was pinned (a default), the default is off. */
export function unpinAgent(spaceId: string, handle: string, source: AddressSource): void {
  const cur = useAddressing.getState().bySpace[spaceId] ?? EMPTY_ADDRESS;
  if (source === "pinned") setAddress(spaceId, { ...cur, pinned: cur.pinned.filter((h) => h !== handle), off: cur.pinned.filter((h) => h !== handle).length === 0 });
  else setAddress(spaceId, { ...cur, pinned: [], off: true });
}

/** Tests start clean. */
export function resetAddressing(): void {
  useAddressing.setState({ bySpace: {} });
}

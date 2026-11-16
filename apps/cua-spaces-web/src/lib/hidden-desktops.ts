// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The computers' desktops a person hid from Home (Hide on a desktop tile's hover). A per-viewer convenience: kept
 * on this device, for each account apart (and once for signed out), so hiding one on a shared Mac, or under
 * another account, never hides it for someone else. Every way to hide has a way back: "N hidden · Show" under
 * Home's computers, the computer's own "Show on Home" switch, and turning a desktop's sharing on.
 */

import { useCallback, useMemo, useSyncExternalStore } from "react";

import { useSession } from "@/bridge";

/** The key before hiding was kept per account; read once, into the first account that looks. */
const LEGACY_KEY = "cua.hiddenDesktops";
const PREFIX = "cua.hiddenDesktops:";
/** The account a key belongs to: the signed-in identity, or `signed-out`. */
const scope = (account: string | null | undefined) => account || "signed-out";
const keyOf = (account: string | null | undefined) => `${PREFIX}${scope(account)}`;

const NONE: ReadonlySet<string> = new Set();
const listeners = new Set<() => void>();
const cache = new Map<string, ReadonlySet<string>>();

function parse(raw: string | null | undefined): string[] | null {
  if (!raw) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : null;
  } catch {
    return null;
  }
}

/** The desktops `account` hid on this device. */
export function readHidden(account: string | null | undefined): ReadonlySet<string> {
  const key = keyOf(account);
  const hit = cache.get(key);
  if (hit) return hit;
  let ids: string[] = [];
  try {
    const store = globalThis.localStorage;
    const own = parse(store?.getItem(key));
    if (own) ids = own;
    else {
      // Hidden before this was kept per account: they move to this account, so they can be shown again.
      const legacy = parse(store?.getItem(LEGACY_KEY));
      if (legacy?.length) {
        ids = legacy;
        store?.setItem(key, JSON.stringify(ids));
      }
      store?.removeItem(LEGACY_KEY);
    }
  } catch {
    // No storage: nothing hidden.
  }
  const set: ReadonlySet<string> = new Set(ids);
  cache.set(key, set);
  return set;
}

function write(account: string | null | undefined, next: Set<string>): void {
  const key = keyOf(account);
  cache.set(key, next);
  try {
    if (next.size) globalThis.localStorage?.setItem(key, JSON.stringify([...next]));
    else globalThis.localStorage?.removeItem(key);
  } catch {
    // Not kept past this window.
  }
  for (const l of [...listeners]) l();
}

/** Hides a desktop from Home for `account`. */
export const hideDesktop = (account: string | null | undefined, id: string): void => write(account, new Set([...readHidden(account), id]));

/** Shows one hidden desktop again for `account`. */
export function showDesktop(account: string | null | undefined, id: string): void {
  const now = readHidden(account);
  if (!now.has(id)) return;
  const next = new Set(now);
  next.delete(id);
  write(account, next);
}

/** Shows every desktop `account` hid. */
export function showHiddenDesktops(account: string | null | undefined): void {
  if (readHidden(account).size) write(account, new Set());
}

/** Forgets what was read (tests). */
export function resetHiddenDesktops(): void {
  cache.clear();
  for (const l of [...listeners]) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export interface HiddenDesktops {
  /** The desktops hidden from Home on this device for the signed-in account. */
  hidden: ReadonlySet<string>;
  hide(id: string): void;
  show(id: string): void;
  showAll(): void;
}

/** The hidden desktops of the account signed in now (signed out has its own). */
export function useHiddenDesktops(): HiddenDesktops {
  const session = useSession();
  const known = session.data !== undefined;
  const account = session.data?.signedIn ? (session.data.identity ?? null) : null;
  // Until the session is read, nothing is hidden (and nothing older is moved to the wrong account).
  const get = useCallback(() => (known ? readHidden(account) : NONE), [known, account]);
  const hidden = useSyncExternalStore(subscribe, get, get);
  return useMemo(
    () => ({
      hidden,
      hide: (id: string) => hideDesktop(account, id),
      show: (id: string) => showDesktop(account, id),
      showAll: () => showHiddenDesktops(account),
    }),
    [hidden, account],
  );
}

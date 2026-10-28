// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The desktop rows a person hid from the Spaces list (Hide on a row's hover). A per-viewer
 * convenience kept in this window's storage: "Show hidden" in the list's overflow, and
 * turning a desktop's sharing on, bring them back.
 */

import { useSyncExternalStore } from "react";

const KEY = "cua.hiddenDesktops";
const listeners = new Set<() => void>();
let cache: ReadonlySet<string> | null = null;

function read(): ReadonlySet<string> {
  if (cache) return cache;
  let ids: string[] = [];
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    if (Array.isArray(parsed)) ids = parsed.filter((x): x is string => typeof x === "string");
  } catch {
    // No storage, or not ours: nothing hidden.
  }
  cache = new Set(ids);
  return cache;
}

function write(next: Set<string>): void {
  cache = next;
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify([...next]));
  } catch {
    // Not kept past this window.
  }
  for (const l of [...listeners]) l();
}

/** Hides a desktop row. */
export const hideDesktop = (id: string): void => write(new Set([...read(), id]));

/** Shows one hidden desktop row again. */
export function showDesktop(id: string): void {
  if (!read().has(id)) return;
  const next = new Set(read());
  next.delete(id);
  write(next);
}

/** Shows every hidden desktop row again. */
export function showHiddenDesktops(): void {
  if (read().size) write(new Set());
}

/** Forgets what was kept (tests). */
export function resetHiddenDesktops(): void {
  cache = null;
  for (const l of [...listeners]) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/** The hidden desktop rows' ids; a new set whenever it changes. */
export function useHiddenDesktops(): ReadonlySet<string> {
  return useSyncExternalStore(subscribe, read, read);
}

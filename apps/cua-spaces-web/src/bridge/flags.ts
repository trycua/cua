// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Feature flags for the web UI. A flag takes its default (`DEFAULTS`: off) unless something sets it, in this order:
 *
 * 1. this device's override (`localStorage` `cua.flag.<name>` = `1` or `0`),
 *    which is how a dev build, a tester or Settings → Advanced → Labs forces one (an explicit
 *    `0` stays off);
 * 2. the build's default (`VITE_CUA_FLAG_<NAME>` = `1`);
 * 3. the flag's default.
 *
 * A flag only decides what the UI draws. Plans and limits are enforced by the
 * relay and the control plane, never by a flag.
 */

import { useSyncExternalStore } from "react";

/**
 * - `spaces_tree`: Spaces as a tree and a default Space (CUA-3413): your
 *   computers and the Spaces made on them in one list (the sidebar and the
 *   Spaces page; Machines leaves the sidebar, its page still opens), and Home's
 *   "in" picker starts on your default Space instead of a new Linux computer.
 *
 * The new Home and the new setup used to be flags here; they are the only Home and the only
 * first run now, and a stored override of either is ignored.
 */
export type FlagName = "spaces_tree";

/** What a device that never touched a switch has. `cua-spaces-ffi` has a test that reads this file and compares its flags with the daemon's and the app's experiment lists. */
const DEFAULTS: Record<FlagName, boolean> = { spaces_tree: false };

const key = (name: string) => `cua.flag.${name}`;

const listeners = new Set<() => void>();
const notify = () => {
  for (const l of [...listeners]) l();
};

/** Forces a flag on this device (`null` clears the override). */
export function setFlagOverride(name: FlagName, on: boolean | null): void {
  try {
    if (on === null) globalThis.localStorage?.removeItem(key(name));
    else globalThis.localStorage?.setItem(key(name), on ? "1" : "0");
  } catch {
    // Not kept.
  }
  notify();
}

function envDefault(name: FlagName): boolean | undefined {
  const v = (import.meta.env as Record<string, string | undefined>)[`VITE_CUA_FLAG_${name.toUpperCase()}`];
  return v === undefined || v === "" ? undefined : v === "1" || v === "true";
}

/** Whether a flag is on now. */
export function flagOn(name: FlagName): boolean {
  let local: string | null = null;
  try {
    local = globalThis.localStorage?.getItem(key(name)) ?? null;
  } catch {
    // No storage: no override.
  }
  if (local === "1") return true;
  if (local === "0") return false;
  return envDefault(name) ?? DEFAULTS[name];
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  const onStorage = (e: StorageEvent) => {
    if (e.key === null || e.key.startsWith("cua.flag.")) l();
  };
  globalThis.addEventListener?.("storage", onStorage);
  return () => {
    listeners.delete(l);
    globalThis.removeEventListener?.("storage", onStorage);
  };
};

export function useFlag(name: FlagName): boolean {
  return useSyncExternalStore(subscribe, () => flagOn(name), () => DEFAULTS[name]);
}

/** Spaces as a tree is on. */
export function useSpacesTree(): boolean {
  return useFlag("spaces_tree");
}

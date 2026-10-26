// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Feature flags for the web UI. A flag is off unless something turns it on,
 * in this order:
 *
 * 1. this device's override (`localStorage` `cua.flag.<name>` = `1` or `0`),
 *    which is how a dev build or a tester forces one;
 * 2. the build's default (`VITE_CUA_FLAG_<NAME>` = `1`);
 * 3. the remote value, when a source has set one (`setRemoteFlags`: the
 *    PostHog feature-flag payload, once the shells pass it in);
 * 4. off.
 *
 * A flag only decides what the UI draws. Plans and limits are enforced by the
 * relay and the control plane, never by a flag.
 */

import { useSyncExternalStore } from "react";

/**
 * - `upsell_cards`: the cards that offer Pro or Teams when a limit stops you.
 * - `spaces_ask_first_home`: the "Ask first" Home experiment (CUA-2346): a
 *   Home page that asks what you want done, starts it on a fresh computer and
 *   follows it as a conversation. Settings → Advanced → Labs turns it on for
 *   this device.
 */
export type FlagName = "upsell_cards" | "spaces_ask_first_home";

const DEFAULTS: Record<FlagName, boolean> = { upsell_cards: false, spaces_ask_first_home: false };

const key = (name: FlagName) => `cua.flag.${name}`;
let remote: Partial<Record<FlagName, boolean>> = {};
const listeners = new Set<() => void>();
const notify = () => {
  for (const l of [...listeners]) l();
};

/** Takes the remote flag values (replacing the last ones). */
export function setRemoteFlags(values: Partial<Record<FlagName, boolean>>): void {
  remote = { ...values };
  notify();
}

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
  return envDefault(name) ?? remote[name] ?? DEFAULTS[name];
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

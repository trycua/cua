// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The phone's own lock: Face ID, Touch ID or the phone's passcode, asked when Cua Spaces opens, when it comes back
 * after a minute away, and before the desktop of a computer is opened if the last unlock is older than five minutes.
 * It is on by default (Settings can turn it off). A phone with no passcode cannot be asked anything, so there the lock
 * opens by itself and Settings says why. The decision (`needsUnlock`) is plain so it can be tested; the system prompt
 * is `askUnlock`.
 */

import * as LocalAuthentication from "expo-local-authentication";
import { create } from "zustand";

/** The app locks again after being away this long. */
export const RELOCK_AFTER_MS = 60_000;
/** The desktop of a computer needs an unlock no older than this. */
export const VIEWER_FRESH_MS = 5 * 60_000;

/** Whether an unlock is needed now. `unlockedAt` is null before the first unlock (and after relocking). */
export function needsUnlock(opts: { required: boolean; unlockedAt: number | null; now: number; maxAgeMs?: number }): boolean {
  if (!opts.required) return false;
  if (opts.unlockedAt === null) return true;
  return opts.maxAgeMs !== undefined && opts.now - opts.unlockedAt > opts.maxAgeMs;
}

/** Whether this phone can ask at all (Face ID, Touch ID or a passcode is set). */
export async function lockAvailable(): Promise<boolean> {
  try {
    return (await LocalAuthentication.getEnrolledLevelAsync()) !== LocalAuthentication.SecurityLevel.NONE;
  } catch {
    return false;
  }
}

export type UnlockResult = "unlocked" | "cancelled" | "unavailable";

/** The system prompt. "unavailable": nothing can be asked on this phone, so the lock lets it through. */
export async function askUnlock(reason: string): Promise<UnlockResult> {
  if (!(await lockAvailable())) return "unavailable";
  try {
    const r = await LocalAuthentication.authenticateAsync({ promptMessage: reason, cancelLabel: "Cancel", fallbackLabel: "Use passcode" });
    return r.success ? "unlocked" : "cancelled";
  } catch {
    return "cancelled";
  }
}

/**
 * The unlock before something that matters: answering an agent, taking control of a desktop, signing out. Asks
 * only when the lock is required and the last unlock is older than `maxAgeMs`. False: the person did not unlock.
 */
export async function ensureUnlocked(reason: string, required: boolean, maxAgeMs: number = VIEWER_FRESH_MS): Promise<boolean> {
  const { unlockedAt, markUnlocked } = useLock.getState();
  const now = Date.now();
  if (!needsUnlock({ required, unlockedAt, now, maxAgeMs })) return true;
  const r = await askUnlock(reason);
  if (r === "cancelled") return false;
  markUnlocked(Date.now());
  return true;
}

interface LockState {
  unlockedAt: number | null;
  leftAt: number | null;
  markUnlocked(now: number): void;
  /** The app went to the background (`now`), or came back: relocks after [`RELOCK_AFTER_MS`] away. */
  left(now: number): void;
  returned(now: number): void;
  lock(): void;
}

export const useLock = create<LockState>((set, get) => ({
  unlockedAt: null,
  leftAt: null,
  markUnlocked: (now) => set({ unlockedAt: now }),
  left: (now) => set({ leftAt: now }),
  returned(now) {
    const { leftAt } = get();
    set({ leftAt: null, ...(leftAt !== null && now - leftAt > RELOCK_AFTER_MS ? { unlockedAt: null } : {}) });
  },
  lock: () => set({ unlockedAt: null }),
}));

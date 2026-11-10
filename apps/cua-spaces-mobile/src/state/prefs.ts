// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AsyncStorage from "@react-native-async-storage/async-storage";
import { create } from "zustand";

interface Prefs {
  /** Local notifications for "needs you" (the only kind until a push path exists). */
  notifyNeedsYou: boolean;
  notifyDone: boolean;
  /** Sample builds only: pretend this phone is offline. */
  sampleOffline: boolean;
  /** Face ID or the passcode when the app opens and before a computer's desktop (on unless turned off). */
  requireUnlock: boolean;
  ready: boolean;
  set(p: Partial<Pick<Prefs, "notifyNeedsYou" | "notifyDone" | "sampleOffline" | "requireUnlock">>): void;
  hydrate(): Promise<void>;
}

const KEY = "cua-spaces-mobile.prefs.v1";

export const usePrefs = create<Prefs>((set, get) => ({
  notifyNeedsYou: true,
  notifyDone: false,
  sampleOffline: false,
  requireUnlock: true,
  ready: false,
  set(p) {
    set(p);
    const { notifyNeedsYou, notifyDone, sampleOffline, requireUnlock } = get();
    AsyncStorage.setItem(KEY, JSON.stringify({ notifyNeedsYou, notifyDone, sampleOffline, requireUnlock })).catch(() => {});
  },
  async hydrate() {
    try {
      const raw = await AsyncStorage.getItem(KEY);
      const saved = raw ? (JSON.parse(raw) as Partial<Pick<Prefs, "notifyNeedsYou" | "notifyDone" | "sampleOffline" | "requireUnlock">>) : {};
      set({
        notifyNeedsYou: saved.notifyNeedsYou ?? true,
        notifyDone: saved.notifyDone ?? false,
        sampleOffline: saved.sampleOffline ?? false,
        requireUnlock: saved.requireUnlock ?? true,
        ready: true,
      });
    } catch {
      set({ ready: true });
    }
  },
}));

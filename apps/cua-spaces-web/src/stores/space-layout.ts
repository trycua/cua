// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

import {
  DEFAULT_PREFS,
  migrateLegacySplit,
  SPACE_LAYOUT_STORAGE_KEY,
  sanitizePrefs,
  type Dock,
  type SectionId,
  type SpaceLayoutPrefs,
} from "@/lib/space-layout";

interface SpaceLayoutState extends SpaceLayoutPrefs {
  setDock: (dock: Dock) => void;
  setSplit: (dock: Dock, fraction: number) => void;
  /** Dock and size in one write (a drop). */
  setLayout: (dock: Dock, fraction: number) => void;
  toggleSection: (id: SectionId) => void;
}

/**
 * The Space page's layout, remembered per person on this device with the
 * rest of the app's UI prefs (same storage as `stores/ui.ts`): where the
 * desktop is docked, how big each split is, and which side-panel sections
 * are folded. One layout for every Space.
 */
export const useSpaceLayoutStore = create<SpaceLayoutState>()(
  persist(
    (set) => ({
      ...DEFAULT_PREFS,
      setDock: (dock) => set({ dock }),
      setSplit: (dock, fraction) => set(dock === "top" ? { vSplit: fraction } : { hSplit: fraction }),
      setLayout: (dock, fraction) => set({ dock, ...(dock === "top" ? { vSplit: fraction } : { hSplit: fraction }) }),
      toggleSection: (id) => set((s) => ({ folded: { ...s.folded, [id]: !s.folded[id] } })),
    }),
    {
      name: SPACE_LAYOUT_STORAGE_KEY,
      // Reading the saved layout first carries over a split saved before the layout existed.
      storage: createJSONStorage(() => ({
        getItem: (name) => {
          migrateLegacySplit(localStorage);
          return localStorage.getItem(name);
        },
        setItem: (name, value) => localStorage.setItem(name, value),
        removeItem: (name) => localStorage.removeItem(name),
      })),
      partialize: (s) => ({ dock: s.dock, vSplit: s.vSplit, hSplit: s.hSplit, folded: s.folded }),
      merge: (saved, current) => ({ ...current, ...sanitizePrefs(saved) }),
    },
  ),
);

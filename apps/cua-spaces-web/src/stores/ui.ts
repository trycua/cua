// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { create } from "zustand";
import { persist } from "zustand/middleware";

export const SIDEBAR_MIN = 196;
export const SIDEBAR_MAX = 360;
export const SIDEBAR_DEFAULT = 232;

export function clampSidebarWidth(width: number): number {
  return Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, Math.round(width)));
}

interface UiState {
  sidebarWidth: number;
  sidebarOpen: boolean;
  paletteOpen: boolean;
  setSidebarWidth: (w: number) => void;
  toggleSidebar: () => void;
  setPaletteOpen: (open: boolean) => void;
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      sidebarWidth: SIDEBAR_DEFAULT,
      sidebarOpen: true,
      paletteOpen: false,
      setSidebarWidth: (w) => set({ sidebarWidth: clampSidebarWidth(w) }),
      toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
      setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
    }),
    {
      name: "cua-spaces:ui",
      partialize: (s) => ({ sidebarWidth: s.sidebarWidth, sidebarOpen: s.sidebarOpen }),
    },
  ),
);

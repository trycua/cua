// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The desktop apps' design tokens (apps/cua-spaces-web/src/index.css: the
 * oklch values converted once to sRGB hex), so the phone reads like the Mac
 * and web apps: borderless tiles a step off the page, one 8 px radius for
 * rows, neutral primary actions, Cua Blue only for focus and selection.
 */

export interface Palette {
  background: string;
  foreground: string;
  card: string;
  muted: string;
  mutedForeground: string;
  accent: string;
  primary: string;
  primaryForeground: string;
  border: string;
  input: string;
  brand: string;
  brandStrong: string;
  brandSurface: string;
  success: string;
  warning: string;
  destructive: string;
  tile: string;
  thumb: string;
  /** Diff colours, GitHub's (the app core's threadCards squares). */
  diffAdd: string;
  diffDel: string;
  diffAddSurface: string;
  diffDelSurface: string;
  /** A message the reader sent. */
  bubbleMine: string;
  bubbleMineText: string;
  /** A message someone else (or an agent) sent. */
  bubbleOther: string;
  /** The page behind the grouped lists. */
  grouped: string;
  overlay: string;
}

export const light: Palette = {
  background: "#f7f8fa",
  foreground: "#181b1f",
  card: "#ffffff",
  muted: "#ebedef",
  mutedForeground: "#656970",
  accent: "#e6e8ec",
  primary: "#1f2228",
  primaryForeground: "#fafafa",
  border: "#dcdee2",
  input: "#d5d7dc",
  brand: "#5ec0e8",
  brandStrong: "#117faa",
  brandSurface: "#5ec0e824",
  success: "#259f56",
  warning: "#c47f00",
  destructive: "#d73431",
  tile: "#eeeff2",
  thumb: "#e6e8eb",
  diffAdd: "#1a7f37",
  diffDel: "#cf222e",
  diffAddSurface: "#dafbe1",
  diffDelSurface: "#ffebe9",
  bubbleMine: "#1f2228",
  bubbleMineText: "#fafafa",
  bubbleOther: "#ebedef",
  grouped: "#f7f8fa",
  overlay: "#00000066",
};

export const dark: Palette = {
  background: "#16181c",
  foreground: "#e9ebef",
  card: "#1e2025",
  muted: "#24262b",
  mutedForeground: "#9498a0",
  accent: "#2b2e34",
  primary: "#e9ebef",
  primaryForeground: "#16181d",
  border: "#ffffff17",
  input: "#ffffff21",
  brand: "#5ec0e8",
  brandStrong: "#73cbf0",
  brandSurface: "#5ec0e829",
  success: "#4dbf74",
  warning: "#edb345",
  destructive: "#f05653",
  tile: "#1d2025",
  thumb: "#202328",
  diffAdd: "#3fb950",
  diffDel: "#f85149",
  diffAddSurface: "#12261a",
  diffDelSurface: "#2d1517",
  bubbleMine: "#e9ebef",
  bubbleMineText: "#16181d",
  bubbleOther: "#24262b",
  grouped: "#16181c",
  overlay: "#000000aa",
};

export const radius = { sm: 4, md: 6, lg: 8, xl: 12, xxl: 16, pill: 999 } as const;
export const space = { xs: 4, sm: 8, md: 12, lg: 16, xl: 24, xxl: 32 } as const;

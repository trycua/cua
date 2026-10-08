// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Windows/Linux title bar overlay's colours. The overlay is drawn by
// Electron above the page, so the page's dialog backdrop can't cover it: when
// a dialog opens, the shell paints the overlay in the colour the backdrop
// gives the top bar beneath it. Pure, so it is tested without Electron.

/** The marker the web UI puts on a dialog backdrop
 * (apps/cua-spaces-web/src/components/ui/dialog.tsx, alert-dialog.tsx). */
export const DIM_ATTRIBUTE = "data-window-dim";

/** Preload -> main: `true` while any backdrop is on the page. */
export const DIM_CHANNEL = "cua-desktop:dim";

// The web UI's `--background` and `--foreground` (index.css), in sRGB.
export const BACKGROUND = { dark: "#16181c", light: "#f7f8fa" } as const;
export const SYMBOL = { dark: "#e9ebef", light: "#181b1f" } as const;

// The backdrop: `bg-black/25`, `dark:bg-black/40`.
export const BACKDROP_ALPHA = { dark: 0.4, light: 0.25 } as const;

type Theme = keyof typeof BACKGROUND;

/** `hex` under black at `alpha`. */
export function darken(hex: string, alpha: number): string {
  const n = Number.parseInt(hex.slice(1), 16);
  const channel = (shift: number) =>
    Math.round(((n >> shift) & 0xff) * (1 - alpha))
      .toString(16)
      .padStart(2, "0");
  return `#${channel(16)}${channel(8)}${channel(0)}`;
}

/** The overlay's `color` and `symbolColor` for `theme`, under a backdrop or not. */
export function overlayColors(theme: Theme, dimmed: boolean): { color: string; symbolColor: string } {
  if (!dimmed) return { color: BACKGROUND[theme], symbolColor: SYMBOL[theme] };
  const a = BACKDROP_ALPHA[theme];
  return { color: darken(BACKGROUND[theme], a), symbolColor: darken(SYMBOL[theme], a) };
}

export interface OverlayStyle {
  color?: string;
  symbolColor?: string;
  height: number;
}

/**
 * The `setTitleBarOverlay` calls that show `colors`: `now`, then `next` a
 * frame later. On Windows the caption buttons were seen keeping their old
 * colours when only the colours changed (the strip stayed bright over a
 * dialog's backdrop), so they are also laid out again: a pixel shorter, then
 * back, which repaints each button. Linux repaints the whole frame itself.
 */
export function overlaySteps(
  platform: NodeJS.Platform,
  colors: { color: string; symbolColor: string },
  height: number,
): { now: OverlayStyle[]; next: OverlayStyle | null } {
  const style = { ...colors, height };
  if (platform !== "win32") return { now: [style], next: null };
  return { now: [{ ...style, height: height - 1 }], next: style };
}

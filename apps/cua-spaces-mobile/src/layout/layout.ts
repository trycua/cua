// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useWindowDimensions } from "react-native";

/**
 * The layout is chosen by the window's width (and orientation), never by the
 * device, so a folded Duo, an iPad in Split View and a phone in landscape all
 * land on the right one:
 *
 * - `phone`: under 600 pt. One column; a Space opens as a screen.
 * - `split`: 600 to 1099 pt and taller than wide (an iPad portrait). The
 *   Spaces list beside the thread, the live desktop stacked above the chat.
 * - `duo`: 600 to 1099 pt and wider than tall (a Duo opened out, an iPad
 *   landscape in a narrow window). The chat on the left half, the desktop on
 *   the right half; the Spaces list is the screen behind.
 * - `triple`: 1100 pt and wider (an iPad landscape). Spaces list, chat and
 *   desktop side by side.
 */
export type LayoutClass = "phone" | "split" | "duo" | "triple";

export function layoutFor(width: number, height: number): LayoutClass {
  if (width < 600) return "phone";
  if (width >= 1100) return "triple";
  return width > height ? "duo" : "split";
}

export function useLayout(): LayoutClass {
  const { width, height } = useWindowDimensions();
  return layoutFor(width, height);
}

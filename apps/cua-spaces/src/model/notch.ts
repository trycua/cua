// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The notch's shared decisions (libs/cua/crates/cua-spaces-app-core
 * `notch`), for the components that draw it: the "N Spaces" tab, the
 * activity indicator left of the notch, the switcher's search and each
 * tile's OS icon. The SwiftUI app reads the same functions through UniFFI.
 */
import { core } from "../core";
import type { Space, SpaceOs } from "./types";

/** The tab's two rows: the count over the word. */
export interface NotchTab {
  count: string;
  word: string;
}

export type NotchActivityKind = "transfer" | "remote-access" | "keyvault" | "hotspot" | "provisioning" | "deleting";

/** The indicator left of the closed notch. */
export interface NotchActivity {
  kind: NotchActivityKind;
  label: string;
  /** The symbol to draw (the hotspot), else a ring. */
  symbol: string | null;
  /** Real progress in thousandths (a transfer with a known total). */
  permille: number | null;
  /** Epoch ms the ring's estimate runs from. */
  startedAt: number | null;
  estimateMs: number;
}

export function notchTab(spaces: readonly Space[]): NotchTab {
  return core("notch.tab", { spaces });
}

/** A transfer first, then someone connected to this machine, then the
 * hotspot, then Spaces starting; null when idle. */
export function notchActivity(
  spaces: readonly Space[],
  hotspot: boolean,
  transfer?: { active: boolean; sent?: number; total?: number },
): NotchActivity | null {
  return core("notch.activity", {
    spaces,
    hotspot,
    transfer: transfer?.active ? { sent: transfer.sent, total: transfer.total } : null,
  });
}

/** The ring's estimated fill (thousandths) after `elapsedMs`. */
export function estimatedProgress(elapsedMs: number, estimateMs: number): number {
  return core("notch.progress", { elapsedMs: Math.round(elapsedMs), estimateMs });
}

/** The Spaces matching the switcher's search (order kept). */
export function filterSpaces<T extends Space>(spaces: readonly T[], query: string): T[] {
  if (!query.trim()) return [...spaces];
  const keep = new Set(core<Space[]>("notch.filter", { spaces, query }).map((s) => s.id));
  return spaces.filter((s) => keep.has(s.id));
}

/** A Space's OS icon id (`os-macos`, `os-windows`, `os-ubuntu`, ..., `os-linux`). */
export function osIcon(os: SpaceOs, osName?: string): string {
  return core("notch.osIcon", { os, osName: osName ?? null });
}

const svgCache = new Map<string, string | null>();

/** The icon's artwork as a data URL (a single-color SVG, used as a mask). */
export function osIconUrl(id: string): string | null {
  if (!svgCache.has(id)) {
    const svg = core<string | null>("notch.osIconSvg", { id });
    svgCache.set(id, svg ? `data:image/svg+xml;utf8,${encodeURIComponent(svg)}` : null);
  }
  return svgCache.get(id) ?? null;
}

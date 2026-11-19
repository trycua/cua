// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { CoreClient } from "@/bridge";

export interface PaletteCommand {
  id: string;
  group: string;
  label: string;
  hint?: string;
  shortcut?: string;
  keywords?: string;
  run: () => void;
}

type Rankable = Pick<PaletteCommand, "label" | "hint" | "keywords"> & { group?: string };

/** Without the core: every word somewhere in label, hint or keywords, in order. */
function plainFilter<T extends Rankable>(commands: readonly T[], words: string[]): T[] {
  return commands.filter((c) => {
    const text = `${c.label} ${c.hint ?? ""} ${c.keywords ?? ""}`.toLowerCase();
    return words.every((w) => text.includes(w));
  });
}

/**
 * Commands whose label, hint or keywords fuzzy-match every word of the
 * query, best first, with groups kept together: the core's ranking
 * (`palette.rank`), the same as the SwiftUI app's palette. An empty query
 * returns every command in its original order.
 */
export function filterCommands<T extends Rankable>(commands: readonly T[], query: string, core?: CoreClient): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...commands];
  const entries = commands.map((c) => ({ group: c.group ?? "", label: c.label, hint: c.hint, keywords: c.keywords }));
  const order = core?.tryCall<number[]>("palette.rank", { entries, query });
  return order ? order.map((i) => commands[i]!) : plainFilter(commands, words);
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

export interface PaletteCommand {
  id: string;
  group: string;
  label: string;
  hint?: string;
  shortcut?: string;
  keywords?: string;
  run: () => void;
}

/**
 * How well `query` fuzzy-matches `text`: its characters must appear in order.
 * Matches at the start of a word and runs of consecutive characters score
 * higher, gaps score lower. Returns 0 for no match.
 */
export function fuzzyScore(text: string, query: string): number {
  const t = text.toLowerCase();
  const q = query.toLowerCase();
  if (!q) return 1;
  let score = 0;
  let from = 0;
  let prev = -2;
  for (const ch of q) {
    const at = t.indexOf(ch, from);
    if (at < 0) return 0;
    const wordStart = at === 0 || /[\s\-_/.:]/.test(t[at - 1]!);
    score += 1;
    if (at === prev + 1) score += 4;
    if (wordStart) score += 6;
    score -= Math.min(3, (at - from) * 0.1);
    prev = at;
    from = at + 1;
  }
  if (t.startsWith(q)) score += 10;
  return Math.max(score, 0.01);
}

/** The best score for one query word across label, hint and keywords (label counts most). */
function wordScore(c: Pick<PaletteCommand, "label" | "hint" | "keywords">, word: string): number {
  return Math.max(fuzzyScore(c.label, word) * 1.5, fuzzyScore(c.hint ?? "", word), fuzzyScore(c.keywords ?? "", word));
}

/**
 * Commands whose label, hint or keywords fuzzy-match every word of the query,
 * best first. Groups stay together, ordered by their best match. An empty
 * query returns every command in its original order.
 */
export function filterCommands<T extends Pick<PaletteCommand, "label" | "hint" | "keywords"> & { group?: string }>(
  commands: readonly T[],
  query: string,
): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...commands];
  const scored: { c: T; score: number; index: number }[] = [];
  commands.forEach((c, index) => {
    let score = 0;
    for (const w of words) {
      const s = wordScore(c, w);
      if (s === 0) return;
      score += s;
    }
    scored.push({ c, score, index });
  });
  const groupBest = new Map<string | undefined, number>();
  for (const s of scored) groupBest.set(s.c.group, Math.max(groupBest.get(s.c.group) ?? 0, s.score));
  return scored
    .toSorted(
      (a, b) =>
        groupBest.get(b.c.group)! - groupBest.get(a.c.group)! ||
        // Ties between groups keep the first group's place.
        (a.c.group === b.c.group ? 0 : firstIndex(scored, a.c.group) - firstIndex(scored, b.c.group)) ||
        b.score - a.score ||
        a.index - b.index,
    )
    .map((s) => s.c);
}

function firstIndex<T extends { group?: string }>(scored: readonly { c: T; index: number }[], group: string | undefined): number {
  return scored.find((s) => s.c.group === group)!.index;
}

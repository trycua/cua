// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `@name` in the Space thread's composer: the mention being typed at the
 * caret (the picker's query), putting the picked name in its place, and
 * splitting text so mentions draw as chips. The daemon parses mentions
 * from the posted text itself (DESIGN §4.1); these only drive the page.
 */

/** A name a mention can complete to. */
export interface MentionCandidate {
  /** What goes after `@` (`claude`, `grace`). */
  handle: string;
  /** How the picker shows it (`Claude Code`, `Grace`). */
  label: string;
  kind: "agent" | "person";
}

/** `@` starts a mention at the start or after a space or an opening
 * bracket, never inside a word (`ada@example.com` is not one). */
const BEFORE = /[\s([{"']/;
const HANDLE = /^[\w.-]*$/;

/** The mention the caret is in: where its `@` is and what follows it. */
export function activeMention(text: string, caret: number): { start: number; query: string } | null {
  const at = text.lastIndexOf("@", Math.max(0, caret - 1));
  if (at < 0 || caret <= at) return null;
  if (at > 0 && !BEFORE.test(text[at - 1]!)) return null;
  const query = text.slice(at + 1, caret);
  return HANDLE.test(query) ? { start: at, query } : null;
}

/** The candidates for `query`: those whose handle or a word of their
 * label starts with it, agents first, in the order given. */
export function matchMentions(query: string, candidates: readonly MentionCandidate[]): MentionCandidate[] {
  const q = query.toLowerCase();
  const hit = (c: MentionCandidate) => !q || c.handle.toLowerCase().startsWith(q) || c.label.toLowerCase().split(/\s+/).some((w) => w.startsWith(q));
  const found = candidates.filter(hit);
  return [...found.filter((c) => c.kind === "agent"), ...found.filter((c) => c.kind !== "agent")];
}

/** `text` with the mention at `start` (as far as `caret`) replaced by
 * `@handle ` and where the caret goes after it. */
export function insertMention(text: string, start: number, caret: number, handle: string): { text: string; caret: number } {
  const after = text.slice(caret).replace(/^[\w.-]*/, "");
  const inserted = `@${handle}${after.startsWith(" ") ? "" : " "}`;
  return { text: text.slice(0, start) + inserted + after, caret: start + inserted.length };
}

export type MentionSegment = { text: string; mention: false } | { text: string; mention: true; handle: string };

const MENTION = /@([\w][\w.-]*)/g;

/** `text` cut into plain runs and `@handle` mentions. With `handles`, only
 * those names count (a stray `@` stays plain). A trailing `.` ends a mention. */
export function mentionSegments(text: string, handles?: ReadonlySet<string>): MentionSegment[] {
  const out: MentionSegment[] = [];
  let last = 0;
  for (const m of text.matchAll(MENTION)) {
    const at = m.index!;
    if (at > 0 && !BEFORE.test(text[at - 1]!)) continue;
    const handle = m[1]!.replace(/[.-]+$/, "");
    if (handles && !handles.has(handle.toLowerCase())) continue;
    if (at > last) out.push({ text: text.slice(last, at), mention: false });
    out.push({ text: `@${handle}`, mention: true, handle: handle.toLowerCase() });
    last = at + 1 + handle.length;
  }
  if (last < text.length) out.push({ text: text.slice(last), mention: false });
  return out;
}

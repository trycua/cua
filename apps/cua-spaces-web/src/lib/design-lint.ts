// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The design language as a lint: what the approved Space page rules out.
 *
 * - One radius (8 px, `rounded-lg`). Round only for avatars, dots and the
 *   switch knob (`rounded-full` on something with no horizontal padding).
 * - No pills: `rounded-full` on a padded text chip.
 * - No partial lines: `border-t/b/l/r/x/y` and `divide-*` are for the
 *   sanctioned full-width lines only (GroupedSection's inner separators, a
 *   page-wide header hairline). Those live in components/ui, or carry a
 *   `design-ok:` comment on the line or the line above saying why.
 * - No outlined boxes: `border` plus a radius around content outside the
 *   primitives in components/ui (a GroupedSection is a fill, not an outline).
 * - No "a · b · c" dot-chains in copy.
 *
 * Settings pages (components/settings, routes/settings, settings-group) also
 * keep to `scanSettings`: spacing, headings and buttons come from the shared
 * components, so every tab matches.
 *
 * - No negative top margins (`-mt-*`) to pull a link or button up under a card:
 *   use the section's `footer`, or a GroupedAction row inside the card.
 * - No hand-made headings (`<h2>`): SettingsGroup's title is the one style.
 * - No filled-gray `variant="secondary"` buttons in rows: secondary row
 *   buttons are `size="sm" variant="outline"`. The Agents surfaces
 *   (components/agents, routes/agents) keep to this one too.
 *
 * `scanSource` is pure so the drift-guard test and the audit script share it.
 */

export type DesignRule = "radius" | "pill" | "partial-line" | "outlined-box" | "dot-chain";

export interface DesignFinding {
  rule: DesignRule;
  line: number;
  text: string;
}

const RAW_RADIUS = /(?:^|[\s"'`:])!?rounded(?:-(?:none|xs|sm|md|xl|2xl|3xl|4xl|\[[^\]]*\])|-[tblrse]{1,2}(?:-[a-z0-9[\]]+)?)?(?=[\s"'`]|$)/;
const ROUNDED_FULL = /(?:^|[\s"'`:])rounded-full(?=[\s"'`]|$)/;
const H_PADDING = /(?:^|[\s"'`:])p[xls]?-(?:[1-9]|\[)/;
const PARTIAL_LINE = /(?:^|[\s"'`:])(?:border-[tblrxy](?:-[0-9]+)?|divide-[xy](?:-[0-9]+)?)(?=[\s"'`]|$)/;
const BORDER = /(?:^|[\s"'`:])border(?:-[0-9]+)?(?=[\s"'`]|$)/;
const ANY_RADIUS = /(?:^|[\s"'`:])rounded(?:-[a-z0-9[\]]+)?(?=[\s"'`]|$)/;
const DOT_CHAIN = /\S · \S/;
const DOT_JOIN = /join\(\s*["'`] ?· ?["'`]\s*\)/;
const OK = /design-ok:/;

/** Class-like string literals on a line: "...", '...', `...`. */
function strings(line: string): string[] {
  const out: string[] = [];
  const re = /"([^"\\]*(?:\\.[^"\\]*)*)"|'([^'\\]*(?:\\.[^'\\]*)*)'|`([^`\\]*(?:\\.[^`\\]*)*)`/g;
  for (let m = re.exec(line); m; m = re.exec(line)) out.push(m[1] ?? m[2] ?? m[3] ?? "");
  return out;
}

/** True for a string that looks like Tailwind classes (not copy). */
function isClassList(s: string): boolean {
  return /(?:^|\s)(?:flex|grid|block|inline|rounded|border|bg-|text-|p[xytblr]?-|m[xytblr]?-|gap-|h-|w-|size-|items-|divide-)/.test(s);
}

export function scanSource(source: string, opts: { primitive?: boolean } = {}): DesignFinding[] {
  const findings: DesignFinding[] = [];
  const lines = source.split("\n");
  lines.forEach((line, i) => {
    if (OK.test(line) || (i > 0 && OK.test(lines[i - 1] ?? ""))) return;
    const trimmed = line.trim();
    if (trimmed.startsWith("//") || trimmed.startsWith("*") || trimmed.startsWith("/*")) return;
    const add = (rule: DesignRule) => findings.push({ rule, line: i + 1, text: trimmed.slice(0, 160) });
    for (const s of strings(line)) {
      if (isClassList(s)) {
        if (RAW_RADIUS.test(s)) add("radius");
        if (ROUNDED_FULL.test(s) && H_PADDING.test(s)) add("pill");
        if (!opts.primitive && PARTIAL_LINE.test(s)) add("partial-line");
        if (!opts.primitive && BORDER.test(s) && ANY_RADIUS.test(s) && !/border-transparent|border-0/.test(s)) add("outlined-box");
      } else if (DOT_CHAIN.test(s)) {
        add("dot-chain");
      }
    }
    if (DOT_JOIN.test(line)) add("dot-chain");
    // Copy written straight into JSX text.
    if (!/["'`]/.test(line) && DOT_CHAIN.test(line) && />[^<]*·/.test(line)) add("dot-chain");
  });
  return findings;
}

export function countByRule(findings: DesignFinding[]): Partial<Record<DesignRule, number>> {
  const out: Partial<Record<DesignRule, number>> = {};
  for (const f of findings) out[f.rule] = (out[f.rule] ?? 0) + 1;
  return out;
}

export type SettingsRule = "settings-spacing" | "settings-heading" | "row-button";

export interface SettingsFinding {
  rule: SettingsRule;
  line: number;
  text: string;
}

/**
 * The Settings-only rules (see the header). A `design-ok:` comment exempts a line.
 * `rowButtonsOnly` (the Agents surfaces) checks just the buttons: a small
 * `variant="secondary"` button is a row button and must be outline; a dialog's
 * full-size Cancel keeps its fill.
 */
export function scanSettings(source: string, opts: { rowButtonsOnly?: boolean } = {}): SettingsFinding[] {
  const findings: SettingsFinding[] = [];
  const lines = source.split("\n");
  lines.forEach((line, i) => {
    if (OK.test(line) || (i > 0 && OK.test(lines[i - 1] ?? ""))) return;
    const trimmed = line.trim();
    if (trimmed.startsWith("//") || trimmed.startsWith("*") || trimmed.startsWith("/*")) return;
    const add = (rule: SettingsRule) => findings.push({ rule, line: i + 1, text: trimmed.slice(0, 160) });
    if (!opts.rowButtonsOnly) {
      if (strings(line).some((s) => isClassList(s) && /(?:^|[\s:])-mt-/.test(s))) add("settings-spacing");
      if (/<h2[\s>]/.test(line)) add("settings-heading");
    }
    if (/variant="secondary"/.test(line) && (!opts.rowButtonsOnly || /size="sm"/.test(line))) add("row-button");
  });
  return findings;
}

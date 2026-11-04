// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A small Markdown reader for agent messages: the blocks agents write
 * (paragraphs, headings, lists, quotes, fenced code, rules) and the inline
 * marks inside them (code, bold, italic, links). Built for streaming: a
 * message is split into top-level blocks, and every block but the last is
 * final, so a view that memoizes blocks by their source re-renders only the
 * one still growing. An unclosed fence is a code block that is still open.
 */

export type Block =
  | { type: "paragraph"; text: string }
  | { type: "heading"; level: number; text: string }
  | { type: "code"; lang: string; code: string; open: boolean }
  | { type: "list"; ordered: boolean; start: number; items: string[] }
  | { type: "quote"; text: string }
  | { type: "rule" };

export interface SourceBlock {
  /** The block's own source, for memoizing its rendering. */
  source: string;
  block: Block;
}

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([\w+-]*)/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE = /^ {0,3}([-*_])(\s*\1){2,}\s*$/;
const BULLET = /^ {0,3}[-*+]\s+(.*)$/;
const ORDERED = /^ {0,3}(\d{1,9})[.)]\s+(.*)$/;
const QUOTE = /^ {0,3}>\s?(.*)$/;

const startsBlock = (line: string) =>
  FENCE.test(line) || HEADING.test(line) || RULE.test(line) || BULLET.test(line) || ORDERED.test(line) || QUOTE.test(line);

export function parseBlocks(markdown: string): SourceBlock[] {
  const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
  const out: SourceBlock[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    if (!line.trim()) {
      i++;
      continue;
    }
    const start = i;
    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[1]!;
      const close = new RegExp(`^ {0,3}${marker[0] === "`" ? "`" : "~"}{${marker.length},}\\s*$`);
      const body: string[] = [];
      i++;
      let open = true;
      while (i < lines.length) {
        if (close.test(lines[i]!)) {
          open = false;
          i++;
          break;
        }
        body.push(lines[i]!);
        i++;
      }
      out.push({ source: lines.slice(start, i).join("\n"), block: { type: "code", lang: fence[2] ?? "", code: body.join("\n"), open } });
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      i++;
      out.push({ source: line, block: { type: "heading", level: heading[1]!.length, text: heading[2]! } });
      continue;
    }
    if (RULE.test(line)) {
      i++;
      out.push({ source: line, block: { type: "rule" } });
      continue;
    }
    const bullet = BULLET.exec(line);
    const ordered = ORDERED.exec(line);
    if (bullet || ordered) {
      const isOrdered = !bullet;
      const items: string[] = [];
      while (i < lines.length) {
        const l = lines[i]!;
        const m = isOrdered ? ORDERED.exec(l) : BULLET.exec(l);
        if (m) {
          items.push(isOrdered ? m[2]! : m[1]!);
          i++;
        } else if (l.trim() && /^\s{2,}/.test(l) && items.length) {
          items[items.length - 1] += `\n${l.trim()}`;
          i++;
        } else if (!l.trim() && i + 1 < lines.length && (isOrdered ? ORDERED : BULLET).test(lines[i + 1]!)) {
          i++;
        } else break;
      }
      out.push({
        source: lines.slice(start, i).join("\n"),
        block: { type: "list", ordered: isOrdered, start: ordered ? Number(ordered[1]) : 1, items },
      });
      continue;
    }
    if (QUOTE.test(line)) {
      const body: string[] = [];
      while (i < lines.length) {
        const m = QUOTE.exec(lines[i]!);
        if (!m) break;
        body.push(m[1]!);
        i++;
      }
      out.push({ source: lines.slice(start, i).join("\n"), block: { type: "quote", text: body.join("\n") } });
      continue;
    }
    const body: string[] = [];
    while (i < lines.length && lines[i]!.trim() && (body.length === 0 || !startsBlock(lines[i]!))) {
      body.push(lines[i]!.trim());
      i++;
    }
    out.push({ source: lines.slice(start, i).join("\n"), block: { type: "paragraph", text: body.join("\n") } });
  }
  return out;
}

export type Inline =
  | { type: "text"; text: string }
  | { type: "code"; text: string }
  | { type: "strong"; children: Inline[] }
  | { type: "em"; children: Inline[] }
  | { type: "link"; href: string; children: Inline[] }
  | { type: "break" };

const SAFE_URL = /^(https?:|mailto:)/i;

/** Inline marks. Unclosed marks stay as text, so a half-streamed `**` reads as typed. */
export function parseInline(text: string): Inline[] {
  const out: Inline[] = [];
  let buf = "";
  const flush = () => {
    if (buf) out.push({ type: "text", text: buf });
    buf = "";
  };
  let i = 0;
  while (i < text.length) {
    const c = text[i]!;
    const rest = text.slice(i);
    if (c === "\\" && i + 1 < text.length && /[\\`*_[\]()#+\-.!>]/.test(text[i + 1]!)) {
      buf += text[i + 1];
      i += 2;
      continue;
    }
    if (c === "\n") {
      flush();
      out.push({ type: "break" });
      i++;
      continue;
    }
    if (c === "`") {
      const run = /^`+/.exec(rest)![0];
      const end = text.indexOf(run, i + run.length);
      if (end > i) {
        flush();
        out.push({ type: "code", text: text.slice(i + run.length, end).replace(/^ (.*) $/, "$1") });
        i = end + run.length;
        continue;
      }
    }
    if ((c === "*" || c === "_") && text[i + 1] === c) {
      const end = text.indexOf(c + c, i + 2);
      if (end > i + 2) {
        flush();
        out.push({ type: "strong", children: parseInline(text.slice(i + 2, end)) });
        i = end + 2;
        continue;
      }
    }
    if ((c === "*" || c === "_") && text[i + 1] && text[i + 1] !== " " && text[i + 1] !== c) {
      const prev = text[i - 1];
      const wordInside = c === "_" && prev !== undefined && /\w/.test(prev);
      const end = text.indexOf(c, i + 1);
      if (!wordInside && end > i + 1 && text[end - 1] !== " ") {
        flush();
        out.push({ type: "em", children: parseInline(text.slice(i + 1, end)) });
        i = end + 1;
        continue;
      }
    }
    if (c === "[") {
      const m = /^\[([^\]]+)\]\(([^)\s]+)\)/.exec(rest);
      if (m && SAFE_URL.test(m[2]!)) {
        flush();
        out.push({ type: "link", href: m[2]!, children: parseInline(m[1]!) });
        i += m[0].length;
        continue;
      }
    }
    if (c === "h" && /^https?:\/\//.test(rest) && (i === 0 || /[\s(]/.test(text[i - 1]!))) {
      const m = /^https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"]/.exec(rest);
      if (m) {
        flush();
        out.push({ type: "link", href: m[0], children: [{ type: "text", text: m[0] }] });
        i += m[0].length;
        continue;
      }
    }
    buf += c;
    i++;
  }
  flush();
  return out;
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Reads the app's source for the i18n guards (`*.test.ts` next to this
 * file): user-visible English left in localized files, and the catalog keys
 * the code uses. Node only; never bundled into the page.
 */

import { parse } from "@babel/parser";

/** JSX attributes and object properties whose string the person sees or hears. */
const VISIBLE = new Set(["title", "aria-label", "aria-description", "placeholder", "alt", "label", "description", "help", "tooltip"]);
/** Calls whose first argument is shown (toasts). */
const SHOWN_CALLS = new Set(["toastError", "toastSuccess", "toastInfo", "fail"]);
/** Calls whose first argument is a catalog key. */
const KEY_CALLS = new Set(["t"]);

const hasLetters = (s: string) => /\p{L}/u.test(s);

type Node = { type: string; loc?: { start: { line: number } } | null; [k: string]: unknown };

function walk(node: unknown, visit: (n: Node) => void): void {
  if (Array.isArray(node)) {
    for (const n of node) walk(n, visit);
    return;
  }
  if (!node || typeof node !== "object" || typeof (node as Node).type !== "string") return;
  visit(node as Node);
  for (const [k, v] of Object.entries(node)) {
    if (k === "loc" || k === "start" || k === "end" || k === "extra" || k === "leadingComments" || k === "trailingComments") continue;
    if (v && typeof v === "object") walk(v, visit);
  }
}

const ast = (fileName: string, source: string) =>
  parse(source, { sourceType: "module", plugins: fileName.endsWith(".tsx") ? ["typescript", "jsx"] : ["typescript"], errorRecovery: true });

/** A string literal, or a template literal with no `${}`, as its text. */
function literalText(n: Node | null | undefined): string | null {
  if (!n) return null;
  if (n.type === "StringLiteral") return n.value as string;
  if (n.type === "TemplateLiteral" && (n.expressions as unknown[]).length === 0) {
    return ((n.quasis as { value: { cooked: string } }[])[0]?.value.cooked ?? "") as string;
  }
  return null;
}

const nameOf = (n: Node | undefined): string | null => {
  if (!n) return null;
  if (n.type === "Identifier") return n.name as string;
  if (n.type === "JSXIdentifier") return n.name as string;
  if (n.type === "StringLiteral") return n.value as string;
  if (n.type === "MemberExpression") return nameOf(n.property as Node);
  return null;
};

/** User-visible English in one file, as `file:line: "text"`. */
export function findHardcodedStrings(fileName: string, source: string): string[] {
  const found: string[] = [];
  const report = (n: Node, text: string) => found.push(`${fileName}:${n.loc?.start.line ?? 0}: ${JSON.stringify(text.trim())}`);
  walk(ast(fileName, source).program, (n) => {
    if (n.type === "JSXText" && hasLetters(n.value as string)) report(n, n.value as string);
    if (n.type === "JSXExpressionContainer") {
      const text = literalText(n.expression as Node);
      // `{"…"}` as a child; attribute values are handled below.
      if (text !== null && hasLetters(text) && !(n as { _attr?: boolean })._attr) report(n, text);
    }
    if (n.type === "JSXAttribute" && VISIBLE.has(nameOf(n.name as Node) ?? "")) {
      const v = n.value as Node | null;
      if (v?.type === "JSXExpressionContainer") (v as { _attr?: boolean })._attr = true;
      const text = v?.type === "StringLiteral" ? (v.value as string) : v?.type === "JSXExpressionContainer" ? literalText(v.expression as Node) : null;
      if (text !== null && hasLetters(text)) report(n, text);
    } else if (n.type === "JSXAttribute") {
      const v = n.value as Node | null;
      if (v?.type === "JSXExpressionContainer") (v as { _attr?: boolean })._attr = true;
    }
    if (n.type === "ObjectProperty" && VISIBLE.has(nameOf(n.key as Node) ?? "")) {
      const text = literalText(n.value as Node);
      if (text !== null && hasLetters(text)) report(n, text);
    }
    if (n.type === "CallExpression" && SHOWN_CALLS.has(nameOf(n.callee as Node) ?? "")) {
      const text = literalText((n.arguments as Node[])[0]);
      if (text !== null && hasLetters(text)) report(n, text);
    }
  });
  return found;
}

/** The catalog keys a file names literally (`t("area.screen.element")`). */
export function usedKeys(fileName: string, source: string): { key: string; line: number }[] {
  const out: { key: string; line: number }[] = [];
  walk(ast(fileName, source).program, (n) => {
    if (n.type !== "CallExpression" || !KEY_CALLS.has(nameOf(n.callee as Node) ?? "")) return;
    if ((n.callee as Node).type !== "Identifier") return;
    const key = literalText((n.arguments as Node[])[0]);
    if (key !== null && /^[a-z0-9_]+(\.[a-z0-9_]+)+$/.test(key)) out.push({ key, line: n.loc?.start.line ?? 0 });
  });
  return out;
}

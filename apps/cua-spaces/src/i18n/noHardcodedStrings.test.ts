// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Fails on user-visible English left in components that have been moved to
// i18n. Area PRs add the files or directories they extract to MIGRATED.
// The webview build has no Node types; tests run in Node.
// @ts-expect-error node:fs is untyped here
import { readdirSync, readFileSync, statSync } from "node:fs";
import ts from "typescript";
import { describe, expect, it } from "vitest";

declare const process: { cwd(): string };

/**
 * Paths relative to `src/` (files, or directories scanned recursively) whose
 * strings are all extracted. Add yours when you finish an area.
 */
export const MIGRATED: string[] = [
  "components/Onboarding.tsx",
  "components/InstallerFlow.tsx",
  "components/desktop/OnboardingPage.tsx",
];

/** JSX attributes whose string values the user sees or hears. */
const VISIBLE_ATTRS = new Set(["title", "aria-label", "placeholder", "alt", "label"]);

const hasLetters = (s: string) => /\p{L}/u.test(s);

function listTsx(rel: string, out: string[] = []): string[] {
  const abs = `${process.cwd()}/src/${rel}`;
  if (statSync(abs).isDirectory()) {
    for (const name of readdirSync(abs) as string[]) listTsx(`${rel}/${name}`, out);
  } else if (rel.endsWith(".tsx") && !/\.test\.tsx$/.test(rel)) {
    out.push(rel);
  }
  return out;
}

/** Hard-coded user-visible strings in one source file, as `line: text`. */
export function findHardcodedStrings(fileName: string, source: string): string[] {
  const sf = ts.createSourceFile(fileName, source, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TSX);
  const found: string[] = [];
  const report = (node: ts.Node, text: string) => {
    const { line } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
    found.push(`${fileName}:${line + 1}: ${JSON.stringify(text.trim())}`);
  };
  const visit = (node: ts.Node) => {
    if (ts.isJsxText(node) && hasLetters(node.text)) report(node, node.text);
    if (ts.isJsxAttribute(node) && VISIBLE_ATTRS.has(node.name.getText(sf)) && node.initializer) {
      const init = node.initializer;
      const lit = ts.isStringLiteral(init)
        ? init
        : ts.isJsxExpression(init) && init.expression &&
            (ts.isStringLiteral(init.expression) || ts.isNoSubstitutionTemplateLiteral(init.expression))
          ? init.expression
          : null;
      if (lit && hasLetters(lit.text)) report(node, lit.text);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return found;
}

describe("no hard-coded UI strings in migrated files", () => {
  it("detects JSX text and visible attributes", () => {
    const src = `export const A = () => <button title="Save" aria-label={"Close"} data-x="abc">Hello {n}<i>{t("k")}</i></button>;`;
    expect(findHardcodedStrings("a.tsx", src)).toHaveLength(3);
    expect(findHardcodedStrings("b.tsx", `export const B = () => <b title={t("k")}>{t("k")} 42</b>;`)).toEqual([]);
  });

  it("finds none in MIGRATED files", () => {
    const offenders = MIGRATED.flatMap((rel) => listTsx(rel)).flatMap((rel) =>
      findHardcodedStrings(rel, readFileSync(`${process.cwd()}/src/${rel}`, "utf8")),
    );
    expect(offenders).toEqual([]);
  });
});

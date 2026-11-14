// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import ts from "typescript";

import { FIXTURES } from "./helpers/core";

/**
 * House wording: "computers" for the user's computers, never "machines" or "devices", and never a middle dot as a separator in UI text.
 * The check reads every string the app can show: string and template literals, JSX text, the core fixtures
 * and the sample data. What is not UI (protocol names, identifiers, headers, the CLI command a person is told
 * to run) is listed with a reason.
 */

const ROOT = join(__dirname, "..");
const MIDDLE_DOTS = /[·•∙⋅‧・]/;
const BANNED = /\b(devices?|machines?)\b/i;
/** The words of the other apps' core (not the phone module's) still say "machines" until that rename lands; "devices" and the dot never. */
const CORE_BANNED = /\b(devices?)\b/i;
const PHONE_METHODS = /^(startTask|outbox|cache|approvals|sendMode|voice|push|diff|directory|placement)\./;

// Not UI wording: a header name, a protocol field, a CLI command the person types.
const ALLOWED_SNIPPETS = [
  "x-cua-device-session",
  "cua devices approve",
  "device_id",
  "device session",
  "cua-device-",
  "dev_",
  "/v1/devices",
  "device-key",
  "machine_id",
];
// Identifiers that are not shown: the tool-call category key (`computer`) in the group phrase table.
const ALLOWED_EXACT = new Set<string>(["machine", "machines", "machine:", "/v1/machines", "/v1/machines/"]);

function walk(dir: string, out: string[] = []): string[] {
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (f === "node_modules" || f === "gen" || f === "ios" || f === "android" || f === "__tests__" || f === "core") continue;
    if (statSync(p).isDirectory()) walk(p, out);
    else if (/\.(tsx?|json)$/.test(f) && !f.endsWith(".d.ts")) out.push(p);
  }
  return out;
}

function stringsOf(file: string): string[] {
  const text = readFileSync(file, "utf8");
  if (file.endsWith(".json")) return [text];
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const out: string[] = [];
  const visit = (n: ts.Node) => {
    if (ts.isImportDeclaration(n) || ts.isExportDeclaration(n)) return;
    if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n)) out.push(n.text);
    else if (ts.isTemplateHead(n) || ts.isTemplateMiddle(n) || ts.isTemplateTail(n)) out.push(n.text);
    else if (ts.isJsxText(n)) out.push(n.getText());
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return out;
}

const scrub = (s: string) => (ALLOWED_EXACT.has(s.trim()) ? "" : ALLOWED_SNIPPETS.reduce((a, k) => a.split(k).join(""), s));

describe("UI wording", () => {
  const files = walk(join(ROOT, "src")).concat(walk(join(ROOT, "app")));

  it("has no middle dot separator in any string the app can show", () => {
    const hits = files.flatMap((f) =>
      stringsOf(f)
        .filter((s) => MIDDLE_DOTS.test(s))
        .map((s) => `${relative(ROOT, f)}: ${s.slice(0, 60)}`),
    );
    expect(hits).toEqual([]);
  });

  it("says computers, never machines or devices", () => {
    const hits = files.flatMap((f) =>
      stringsOf(f)
        .filter((s) => BANNED.test(scrub(s)))
        .map((s) => `${relative(ROOT, f)}: ${s.slice(0, 60)}`),
    );
    expect(hits).toEqual([]);
  });

  it("holds for the core's words (every fixture, the push card included)", () => {
    const words = JSON.stringify([FIXTURES.map((f) => f.result)]);
    expect(MIDDLE_DOTS.test(words)).toBe(false);
    expect(CORE_BANNED.test(words)).toBe(false);
    // The phone module's own words are held to the whole rule (tool-row phrases are the desktop's `agents_label`, pinned equal to it by a Rust test, so they follow its rename).
    const shown: string[] = [];
    const collect = (v: unknown) => {
      if (typeof v === "string") shown.push(v);
      else if (Array.isArray(v)) v.forEach(collect);
      else if (v && typeof v === "object") Object.values(v).forEach(collect);
    };
    FIXTURES.filter((f) => PHONE_METHODS.test(f.method)).forEach((f) => collect(f.result));
    // "machine" and "machine:<id>" are row identifiers, never shown.
    expect(shown.filter((s) => s !== "machine" && !s.startsWith("machine:") && BANNED.test(s))).toEqual([]);
  });
});

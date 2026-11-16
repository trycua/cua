// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Guards the catalog: valid ICU English, keys in their area's file, every
// key the code names exists, every shipped language has every key with the
// same variables, and no language file carries a key English dropped.
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { IntlMessageFormat } from "intl-messageformat";
import { describe, expect, it } from "vitest";

import { LANGUAGES } from "./languages";
import { usedKeys } from "./source-scan";

const src = `${process.cwd()}/src`;
const root = `${src}/i18n/locales`;

const files = (lng: string) => (existsSync(`${root}/${lng}`) ? readdirSync(`${root}/${lng}`).filter((f) => f.endsWith(".json")) : []);
const read = (lng: string, file: string): Record<string, string> => JSON.parse(readFileSync(`${root}/${lng}/${file}`, "utf8"));
const all = (lng: string) => Object.assign({}, ...files(lng).map((f) => read(lng, f))) as Record<string, string>;

/** The variable names an ICU message uses, or an error. */
function variables(message: string, locale: string): string[] | { error: string } {
  try {
    const names = new Set<string>();
    const walk = (els: { type: number; value?: string; options?: Record<string, { value: unknown[] }>; children?: unknown[] }[]) => {
      for (const el of els) {
        if (el.type >= 1 && el.type <= 6 && el.value) names.add(el.value);
        if (el.options) for (const o of Object.values(el.options)) walk(o.value as never);
        if (el.children) walk(el.children as never);
      }
    };
    walk(new IntlMessageFormat(message, locale).getAst() as never);
    return [...names].sort();
  } catch (e) {
    return { error: String((e as Error).message).split("\n")[0] ?? "invalid" };
  }
}

function sources(rel = ".", out: string[] = []): string[] {
  const abs = `${src}/${rel}`;
  if (statSync(abs).isDirectory()) {
    for (const name of readdirSync(abs)) if (name !== "node_modules") sources(rel === "." ? name : `${rel}/${name}`, out);
  } else if (/\.tsx?$/.test(rel) && !/\.test\.tsx?$/.test(rel) && !/(^|\/)__tests__\//.test(rel)) {
    out.push(rel);
  }
  return out;
}

const english = all("en");

describe("the English catalog", () => {
  it("keeps each key in its area's file and every string valid ICU", () => {
    const problems: string[] = [];
    for (const f of files("en")) {
      const area = f.replace(/\.json$/, "");
      for (const [key, text] of Object.entries(read("en", f))) {
        if (!key.startsWith(`${area}.`)) problems.push(`en/${f}: ${key} belongs in ${key.split(".")[0]}.json`);
        if (!/^[a-z0-9_]+(\.[a-z0-9_]+)+$/.test(key)) problems.push(`en/${f}: ${key} is not area.screen.element`);
        const v = variables(text, "en");
        if ("error" in v) problems.push(`en/${f}: ${key}: ${v.error}`);
      }
    }
    expect(problems).toEqual([]);
  });

  it("has every key the code names", () => {
    const missing = sources()
      .flatMap((rel) => usedKeys(rel, readFileSync(`${src}/${rel}`, "utf8")).map((u) => ({ ...u, rel })))
      .filter((u) => !(u.key in english))
      .map((u) => `${u.rel}:${u.line}: ${u.key}`);
    expect(missing, "Add these keys to src/i18n/locales/en").toEqual([]);
  });
});

describe("the other languages", () => {
  it("are exactly the shipped languages", () => {
    const dirs = readdirSync(root).filter((d) => statSync(`${root}/${d}`).isDirectory());
    expect(dirs.sort()).toEqual(LANGUAGES.map((l) => l.code).sort());
  });

  for (const lang of LANGUAGES.filter((l) => l.code !== "en")) {
    it(`${lang.code}: has every English key, with the same variables, and no other`, () => {
      const theirs = all(lang.code);
      const problems: string[] = [];
      for (const [key, text] of Object.entries(english)) {
        if (!(key in theirs)) {
          problems.push(`${key}: missing (run pnpm i18n:translate)`);
          continue;
        }
        const want = variables(text, "en");
        const got = variables(theirs[key]!, lang.code);
        if ("error" in got) problems.push(`${key}: ${got.error}`);
        else if (!("error" in want) && want.join() !== got.join()) problems.push(`${key}: variables ${got.join()} ≠ ${want.join()}`);
      }
      for (const key of Object.keys(theirs)) if (!(key in english)) problems.push(`${key}: not in English`);
      expect(problems).toEqual([]);
    });
  }
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Guards the generated translations: every string in locales/<lng> must still
// be valid next to its English (ICU, variables, tags, plural categories,
// product names) and no file may carry a key English no longer has. A string
// English added or changed may be missing or one revision behind until the
// translation PR merges, so completeness is not required here.
// @ts-expect-error node:fs is untyped here
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";

// @ts-expect-error plain .mjs shared with the translation script
import { TARGET_LOCALES, analyze, validateTranslation } from "../../scripts/i18n/lib.mjs";
import glossary from "./glossary.json";
import { AVAILABLE_LOCALES, NAMESPACES } from "./index";

declare const process: { cwd(): string };

const root = `${process.cwd()}/src/i18n/locales`;
/** Dotted-key entries of a (possibly nested) catalog. */
const flatten = (obj: Record<string, unknown>, prefix = "", out: Record<string, string> = {}): Record<string, string> => {
  for (const [k, v] of Object.entries(obj)) {
    if (v && typeof v === "object") flatten(v as Record<string, unknown>, `${prefix}${k}.`, out);
    else out[`${prefix}${k}`] = v as string;
  }
  return out;
};
const read = (lng: string, ns: string): Record<string, string> => {
  const p = `${root}/${lng}/${ns}.json`;
  return existsSync(p) ? flatten(JSON.parse(readFileSync(p, "utf8"))) : {};
};

describe("translations", () => {
  it("ship exactly the target languages", () => {
    expect([...AVAILABLE_LOCALES].sort()).toEqual(["en", ...TARGET_LOCALES].sort());
    expect((readdirSync(root) as string[]).sort()).toEqual([...AVAILABLE_LOCALES].sort());
  });

  it("English strings all parse as ICU", () => {
    for (const ns of NAMESPACES) {
      for (const [key, value] of Object.entries(read("en", ns))) {
        expect(analyze(value).error, key).toBeUndefined();
      }
    }
  });

  for (const lng of TARGET_LOCALES as string[]) {
    it(`${lng}: every string is valid and matches an English key`, () => {
      const problems: string[] = [];
      for (const ns of NAMESPACES) {
        const en = read("en", ns);
        for (const [key, translation] of Object.entries(read(lng, ns))) {
          if (!(key in en)) {
            problems.push(`${ns}/${key}: no such English key`);
            continue;
          }
          const v = validateTranslation({
            key,
            source: en[key],
            translation,
            locale: lng,
            doNotTranslate: glossary.doNotTranslate,
          });
          problems.push(...v.errors);
        }
      }
      expect(problems).toEqual([]);
    });
  }
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Fails on user-visible English in localized files (`localized.ts`): JSX
// text, visible attributes (title, aria-label, placeholder, alt, label,
// description), the same names as object properties, and toast titles.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { LOCALIZED } from "./localized";
import { findHardcodedStrings } from "./source-scan";

const src = `${process.cwd()}/src`;

function sources(rel: string, out: string[] = []): string[] {
  const abs = `${src}/${rel}`;
  if (statSync(abs).isDirectory()) {
    for (const name of readdirSync(abs)) sources(rel === "." ? name : `${rel}/${name}`, out);
  } else if (/\.tsx?$/.test(rel) && !/\.test\.tsx?$/.test(rel) && !/(^|\/)__tests__\//.test(rel) && !rel.endsWith(".d.ts")) {
    out.push(rel);
  }
  return out;
}

describe("no hard-coded words in localized files", () => {
  it("finds JSX text, visible attributes and properties, and toast titles", () => {
    const code = `export const A = () => <button title="Save" aria-label={"Close"} data-x="abc">Hello {n}<i>{t("k.x")}</i>{"Bye"}</button>;
const o = { label: "Light", value: "light" };
toastError("Couldn't save")(e);`;
    expect(findHardcodedStrings("a.tsx", code).map((l) => l.split(": ")[1])).toEqual(['"Save"', '"Close"', '"Hello"', '"Bye"', '"Light"', '"Couldn\'t save"']);
    expect(findHardcodedStrings("b.tsx", `export const B = () => <b title={t("k.y")} className="p-2">{t("k.y")} 42 · {name}</b>;`)).toEqual([]);
  });

  it("finds none in LOCALIZED files", () => {
    const offenders = LOCALIZED.flatMap((rel) => sources(rel))
      .filter((rel) => !rel.startsWith("i18n/source-scan"))
      .flatMap((rel) => findHardcodedStrings(rel, readFileSync(`${src}/${rel}`, "utf8")));
    expect(offenders, "Move these into src/i18n/locales/en (see src/i18n/README.md)").toEqual([]);
  });
});

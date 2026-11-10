// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { hostLanguages } from "./host-languages";
import { detectLocale, matchLocale, type UiLanguage } from "./languages";
import { pseudoLocalize } from "./pseudo";

const SHIPPED: UiLanguage[] = [
  { code: "en", endonym: "English", status: "reviewed" },
  { code: "ja", endonym: "日本語", status: "reviewed" },
  { code: "zh-Hans", endonym: "简体中文", status: "reviewed" },
  { code: "pt-BR", endonym: "Português (Brasil)", status: "reviewed" },
  { code: "es", endonym: "Español", status: "reviewed" },
  { code: "de", endonym: "Deutsch", status: "draft" },
];
const codes = SHIPPED.map((l) => l.code);

describe("matchLocale", () => {
  it("maps system tags to the app's languages", () => {
    expect(matchLocale("ja-JP", codes)).toBe("ja");
    expect(matchLocale("zh-CN", codes)).toBe("zh-Hans");
    expect(matchLocale("zh-Hans-CN", codes)).toBe("zh-Hans");
    expect(matchLocale("zh", codes)).toBe("zh-Hans");
    expect(matchLocale("pt-PT", codes)).toBe("pt-BR");
    expect(matchLocale("es-419", codes)).toBe("es");
    expect(matchLocale("en-GB", codes)).toBe("en");
  });

  it("never shows Simplified Chinese for Traditional, nor anything for unknown tags", () => {
    expect(matchLocale("zh-TW", codes)).toBeNull();
    expect(matchLocale("zh-Hant-HK", codes)).toBeNull();
    expect(matchLocale("ko-KR", codes)).toBeNull();
    expect(matchLocale("not a tag!", codes)).toBeNull();
  });
});

describe("detectLocale", () => {
  it("prefers the saved choice, then the system's languages in order, then English", () => {
    expect(detectLocale("es", ["ja-JP"], SHIPPED)).toBe("es");
    expect(detectLocale(null, ["ko-KR", "ja-JP", "en-US"], SHIPPED)).toBe("ja");
    expect(detectLocale(null, ["ko-KR"], SHIPPED)).toBe("en");
    expect(detectLocale("xx", [], SHIPPED)).toBe("en");
  });

  it("never picks a draft language from the system, but keeps it when chosen", () => {
    expect(detectLocale(null, ["de-DE", "en-US"], SHIPPED)).toBe("en");
    expect(detectLocale("de", ["en-US"], SHIPPED)).toBe("de");
  });

  it("keeps the pseudo language", () => {
    expect(detectLocale("pseudo", ["ja"], SHIPPED)).toBe("pseudo");
  });
});

describe("hostLanguages", () => {
  it("reads the SwiftUI host, then Electron, then the browser", () => {
    const navigator = { languages: ["fr-FR"], language: "fr-FR" };
    expect(hostLanguages({ cuaHost: { languages: ["ja-JP", "en-US"] }, cuaDesktop: { languages: ["de"] }, navigator })).toEqual(["ja-JP", "en-US"]);
    expect(hostLanguages({ cuaDesktop: { languages: ["de-DE"] }, navigator })).toEqual(["de-DE"]);
    expect(hostLanguages({ cuaDesktop: { languages: [] }, navigator })).toEqual(["fr-FR"]);
    expect(hostLanguages({ navigator: { language: "es" } })).toEqual(["es"]);
    expect(hostLanguages({})).toEqual([]);
  });
});

describe("pseudoLocalize", () => {
  it("accents, pads and brackets the text", () => {
    const p = pseudoLocalize("Settings");
    expect(p.startsWith("[Šéţţîñĝš ")).toBe(true);
    expect(p.endsWith("]")).toBe(true);
    expect(p.length).toBeGreaterThan("Settings".length * 1.3);
    expect(pseudoLocalize("  ")).toBe("  ");
  });
});

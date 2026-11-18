// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import i18n, { currentUiLocale, detectLocale, matchLocale, t } from "./index";

describe("i18n", () => {
  it("starts in English, synchronously", () => {
    expect(currentUiLocale()).toBe("en");
    expect(t("common.app_name")).toBe("Cua Spaces");
    expect(i18n.t("missing.key", { defaultValue: "x" })).toBe("x");
  });

  it("formats ICU plurals", () => {
    i18n.addResource("en", "spaces", "test.count", "{count, plural, one {# Space} other {# Spaces}}");
    expect(t("spaces:test.count", { count: 1 })).toBe("1 Space");
    expect(t("spaces:test.count", { count: 3 })).toBe("3 Spaces");
    i18n.removeResourceBundle("en", "spaces");
  });

  it("matches tags by language, script and region", () => {
    const all = ["en", "zh-Hans", "pt-BR", "ja"];
    expect(matchLocale("zh-CN", all)).toBe("zh-Hans");
    expect(matchLocale("zh-SG", all)).toBe("zh-Hans");
    expect(matchLocale("zh-TW", all)).toBeNull();
    expect(matchLocale("pt-PT", all)).toBe("pt-BR");
    expect(matchLocale("ja-JP", all)).toBe("ja");
    expect(matchLocale("fr-FR", all)).toBeNull();
    expect(matchLocale("not a tag", all)).toBeNull();
  });

  it("prefers the saved setting, then browser languages, then en", () => {
    const all = ["en", "ja", "es"];
    expect(detectLocale("es", ["ja"], all)).toBe("es");
    expect(detectLocale(null, ["fr", "ja-JP"], all)).toBe("ja");
    expect(detectLocale("xx", ["fr"], all)).toBe("en");
    expect(detectLocale(null, ["ja"])).toBe("ja"); // every launch language is bundled
  });
});

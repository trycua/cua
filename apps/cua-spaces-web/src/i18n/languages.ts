// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The languages the app ships and how a system language maps to one.
 * Design: src/i18n/README.md.
 */

/**
 * A language the app can show. `draft` languages are machine translations a
 * native speaker has not signed off yet: they can be picked by hand (marked
 * "Preview") but are never chosen from the system's languages.
 */
export interface UiLanguage {
  code: string;
  /** Its name in its own language, so anyone can find theirs whatever the
   * app currently shows. */
  endonym: string;
  status: "reviewed" | "draft";
}

/** English is the source and the fallback. Add a language here when its
 * files are under `locales/<code>/`. */
export const LANGUAGES: readonly UiLanguage[] = [{ code: "en", endonym: "English", status: "reviewed" }];

export const FALLBACK_LOCALE = "en";

/** Pseudo-localization (`?lang=pseudo`): accented, padded, bracketed English
 * that shows unextracted strings and clipped layouts. Never offered to users. */
export const PSEUDO_LOCALE = "pseudo";

/** Where the person's choice is kept (absent: follow the system). */
export const LOCALE_STORAGE_KEY = "cua-spaces:locale";

const codes = (langs: readonly UiLanguage[]) => langs.map((l) => l.code);

/**
 * One of `available` for a BCP 47 tag, or null. Matches by language, then
 * script or region: zh-CN, zh-SG and zh are zh-Hans (zh-TW, zh-HK and zh-MO
 * are zh-Hant), any pt is pt-BR, es-419 and es-MX are es.
 */
export function matchLocale(tag: string, available: readonly string[]): string | null {
  let canonical: string;
  try {
    canonical = Intl.getCanonicalLocales(tag)[0] ?? "";
  } catch {
    return null;
  }
  if (!canonical) return null;
  if (available.includes(canonical)) return canonical;
  const [lang = "", ...rest] = canonical.split("-");
  let candidate = lang;
  if (lang === "zh") {
    const traditional = rest.some((p) => p === "Hant" || p === "TW" || p === "HK" || p === "MO");
    candidate = traditional ? "zh-Hant" : "zh-Hans";
  } else if (lang === "pt") {
    candidate = "pt-BR";
  }
  return available.includes(candidate) ? candidate : null;
}

/**
 * The language to show: the saved choice (any shipped language, drafts too),
 * then the system's languages in order (reviewed languages only), then
 * English. `saved` may be the pseudo locale.
 */
export function detectLocale(saved: string | null, systemLanguages: readonly string[], languages: readonly UiLanguage[] = LANGUAGES): string {
  if (saved === PSEUDO_LOCALE) return PSEUDO_LOCALE;
  if (saved) {
    const m = matchLocale(saved, codes(languages));
    if (m) return m;
  }
  const reviewed = codes(languages.filter((l) => l.status === "reviewed"));
  for (const tag of systemLanguages) {
    const m = matchLocale(tag, reviewed);
    if (m) return m;
  }
  return FALLBACK_LOCALE;
}

/** The system language the app would follow with no saved choice. */
export function systemLocale(systemLanguages: readonly string[], languages: readonly UiLanguage[] = LANGUAGES): string {
  return detectLocale(null, systemLanguages, languages);
}

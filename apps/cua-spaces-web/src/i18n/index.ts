// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app's words in the person's language: i18next with ICU messages.
 *
 * Keys are `<area>.<screen_or_component>.<element>`, and the area is the
 * file: `settings.general.language` is in `locales/<lng>/settings.json`.
 * English is bundled with the page; the other languages load on demand (one
 * chunk each) before the first render that needs them. See src/i18n/README.md.
 */

import i18next, { type PostProcessorModule, type TOptions } from "i18next";
import ICU from "i18next-icu";
import { initReactI18next, useTranslation } from "react-i18next";

import { ENGLISH_FILES } from "./english-files";
import { hostLanguages } from "./host-languages";
import { FALLBACK_LOCALE, LANGUAGES, LOCALE_STORAGE_KEY, PSEUDO_LOCALE, detectLocale } from "./languages";
import { pseudoLocalize } from "./pseudo";

export { FALLBACK_LOCALE, LANGUAGES, PSEUDO_LOCALE } from "./languages";

type Catalog = Record<string, string>;

// Vite (the page, Electron, Vitest) defines `import.meta.env`; Playwright's loader does not, and has no `glob`.
const underVite = typeof import.meta.env !== "undefined";
const english: Record<string, Catalog> = underVite ? import.meta.glob<Catalog>("./locales/en/*.json", { eager: true, import: "default" }) : ENGLISH_FILES;
const others: Record<string, () => Promise<Catalog>> = underVite
  ? import.meta.glob<Catalog>(["./locales/*/*.json", "!./locales/en/*.json"], { import: "default" })
  : {};

const fileOf = (path: string) => /\/locales\/([^/]+)\/([^/]+)\.json$/.exec(path);

/** The catalog files, one per area (`common`, `settings`, ...). */
export const NAMESPACES: readonly string[] = Object.keys(english)
  .map((p) => fileOf(p)?.[2] ?? "")
  .filter(Boolean)
  .sort();

const englishResources: Record<string, Catalog> = {};
for (const [path, strings] of Object.entries(english)) {
  const ns = fileOf(path)?.[2];
  if (ns) englishResources[ns] = strings;
}

const pseudo: PostProcessorModule = {
  type: "postProcessor",
  name: "pseudo",
  process: (value: string, _key: string | string[], _options: TOptions, translator: { language?: string }) =>
    translator.language === PSEUDO_LOCALE ? pseudoLocalize(value) : value,
};

void i18next
  .use(new ICU())
  .use(pseudo)
  .use(initReactI18next)
  .init({
    resources: { [FALLBACK_LOCALE]: englishResources },
    lng: FALLBACK_LOCALE,
    fallbackLng: FALLBACK_LOCALE,
    ns: [...NAMESPACES],
    defaultNS: "common",
    // Keys are full dotted names, never nested objects.
    keySeparator: false,
    nsSeparator: false,
    initAsync: false,
    interpolation: { escapeValue: false }, // React escapes
    returnNull: false,
    postProcess: ["pseudo"],
    react: { useSuspense: false },
  });

/** The area (file) of a key: its first segment. */
const nsOf = (key: string): string => {
  const ns = key.split(".")[0] ?? "";
  return NAMESPACES.includes(ns) ? ns : "common";
};

/** Loads a language's files (once) so a switch to it renders at once. */
async function load(lng: string): Promise<void> {
  if (lng === FALLBACK_LOCALE || lng === PSEUDO_LOCALE) return;
  const files = Object.entries(others).filter(([p]) => fileOf(p)?.[1] === lng);
  await Promise.all(
    files.map(async ([path, importer]) => {
      const ns = fileOf(path)?.[2];
      if (ns && !i18next.hasResourceBundle(lng, ns)) i18next.addResourceBundle(lng, ns, await importer(), true, true);
    }),
  );
}

function applyDocumentLanguage(lng: string): void {
  if (typeof document === "undefined") return;
  document.documentElement.lang = lng === PSEUDO_LOCALE ? "en-XA" : lng;
}

/** Shows `lng` (a shipped code or the pseudo locale), after loading it. */
export async function changeLocale(lng: string): Promise<void> {
  await load(lng);
  await i18next.changeLanguage(lng);
  applyDocumentLanguage(lng);
}

/** The saved choice, or null to follow the system. `?lang=` (a code or
 * `pseudo`) overrides it for one page load. */
export function savedLocale(win: { location?: { search: string }; localStorage?: Storage } = globalThis as never): string | null {
  try {
    const forced = new URLSearchParams(win.location?.search ?? "").get("lang");
    if (forced) return forced;
  } catch {
    // No location (tests).
  }
  try {
    return win.localStorage?.getItem(LOCALE_STORAGE_KEY) || null;
  } catch {
    return null;
  }
}

/** Before the first render: picks the language and loads it. */
export function startI18n(): Promise<void> {
  return changeLocale(detectLocale(savedLocale(), hostLanguages(), LANGUAGES));
}

/** The language in use now. */
export function currentLocale(): string {
  return i18next.language ?? FALLBACK_LOCALE;
}

/** Translates outside React. Variables use ICU: `{count, plural, ...}`. */
export function t(key: string, vars?: Record<string, unknown>): string {
  return i18next.t(key, { ...vars, ns: nsOf(key) }) as string;
}

/** React hook: `const t = useT()`. Re-renders when the language changes. */
export function useT(): typeof t {
  // react-i18next's `t` is a new function after a language change, so what
  // the React compiler memoized on it is computed again.
  const { t: translate } = useTranslation(NAMESPACES as string[]);
  return (key, vars) => translate(key, { ...vars, ns: nsOf(key) }) as string;
}

/** A message from the app core: a code and its variables, never English
 * (`message::Msg`). The code is a catalog key. */
export interface Msg {
  code: string;
  vars: Record<string, string>;
}

/** A core message in the current language. */
export function tMsg(msg: Msg): string {
  return t(msg.code, msg.vars);
}

export default i18next;

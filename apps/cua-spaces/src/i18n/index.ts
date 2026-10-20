// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import i18next, { type TOptions } from "i18next";
import ICU from "i18next-icu";
import { initReactI18next, useTranslation } from "react-i18next";

import common from "./locales/en/common.json";
import onboarding from "./locales/en/onboarding.json";
import spaces from "./locales/en/spaces.json";
import sharing from "./locales/en/sharing.json";
import billing from "./locales/en/billing.json";
import settings from "./locales/en/settings.json";
import teleport from "./locales/en/teleport.json";
import agents from "./locales/en/agents.json";
import devices from "./locales/en/devices.json";
import installer from "./locales/en/installer.json";
import errors from "./locales/en/errors.json";

/** One namespace per product area. Keys: `<area>.<screen>.<element>`. */
export const NAMESPACES = [
  "common",
  "onboarding",
  "spaces",
  "sharing",
  "billing",
  "settings",
  "teleport",
  "agents",
  "devices",
  "installer",
  "errors",
] as const;
export type Namespace = (typeof NAMESPACES)[number];

/** Languages whose resources are bundled. Phase 1 ships English only. */
export const AVAILABLE_LOCALES = ["en"] as const;
export const FALLBACK_LOCALE = "en";

/** localStorage key for the Settings language choice (absent = system default). */
export const LOCALE_SETTING_KEY = "cua.settings.locale";

const resources = {
  en: { common, onboarding, spaces, sharing, billing, settings, teleport, agents, devices, installer, errors },
};

/**
 * Map a BCP 47 tag to one of the app's locale codes, or null when the
 * language is unknown. Matches by language, then script or region.
 */
export function matchLocale(tag: string, available: readonly string[] = AVAILABLE_LOCALES): string | null {
  let canonical: string;
  try {
    canonical = Intl.getCanonicalLocales(tag)[0] ?? "";
  } catch {
    return null;
  }
  if (!canonical) return null;
  const [lang = "", ...rest] = canonical.split("-");
  let candidate = canonical;
  if (lang === "zh") {
    const traditional = rest.some((p) => p === "Hant" || p === "TW" || p === "HK" || p === "MO");
    candidate = traditional ? "zh-Hant" : "zh-Hans"; // zh-CN, zh-SG, zh -> Hans
  } else if (lang === "pt") {
    candidate = "pt-BR"; // pt-PT and bare pt -> pt-BR
  } else {
    candidate = lang;
  }
  return available.includes(candidate) ? candidate : null;
}

function savedLocale(): string | null {
  try {
    return window.localStorage?.getItem(LOCALE_SETTING_KEY) || null;
  } catch {
    return null;
  }
}

/** Saved setting, then the OS/browser languages, then English. */
export function detectLocale(
  saved: string | null = savedLocale(),
  languages: readonly string[] = typeof navigator !== "undefined" ? (navigator.languages ?? []) : [],
  available: readonly string[] = AVAILABLE_LOCALES,
): string {
  if (saved) {
    const m = matchLocale(saved, available);
    if (m) return m;
  }
  for (const tag of languages) {
    const m = matchLocale(tag, available);
    if (m) return m;
  }
  return FALLBACK_LOCALE;
}

// Synchronous init with bundled resources: the first render is already in
// the right language, so there is no flash.
void i18next
  .use(new ICU())
  .use(initReactI18next)
  .init({
    resources,
    lng: detectLocale(),
    fallbackLng: FALLBACK_LOCALE,
    supportedLngs: [...AVAILABLE_LOCALES],
    ns: [...NAMESPACES],
    defaultNS: "common",
    initAsync: false,
    interpolation: { escapeValue: false }, // React escapes
    returnNull: false,
    react: { useSuspense: false },
  });

/** The locale in use right now (not the OS locale). */
export function currentUiLocale(): string {
  return i18next.resolvedLanguage ?? i18next.language ?? FALLBACK_LOCALE;
}

/** Non-React translate. Use `ns:key` or pass `{ ns }`. */
export function t(key: string, options?: TOptions): string {
  return i18next.t(key, options) as string;
}

/** React hook: `const t = useT("sharing")`. Re-renders on locale change. */
export function useT(ns: Namespace | Namespace[] = "common") {
  return useTranslation(ns).t;
}

export default i18next;

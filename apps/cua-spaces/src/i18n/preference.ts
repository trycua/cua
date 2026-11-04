// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import i18next from "i18next";

import { AVAILABLE_LOCALES, LOCALE_SETTING_KEY, detectLocale } from "./index";
import { reportUiLocale } from "./reportLocale";

/** Each language in its own language, so a user can find theirs whatever the UI says. */
export const LOCALE_ENDONYMS: Record<(typeof AVAILABLE_LOCALES)[number], string> = {
  en: "English",
  "zh-Hans": "简体中文",
  ko: "한국어",
  "pt-BR": "Português (Brasil)",
  ja: "日本語",
  es: "Español",
};

/** The Settings choice: a locale code, or "system" when none is saved. */
export function savedLocaleChoice(): string {
  try {
    const saved = window.localStorage?.getItem(LOCALE_SETTING_KEY);
    return saved && (AVAILABLE_LOCALES as readonly string[]).includes(saved) ? saved : "system";
  } catch {
    return "system";
  }
}

/**
 * Save the Settings language choice (`"system"` clears it), switch the UI
 * live and tell telemetry which language is now in use.
 */
export async function setLocaleChoice(choice: string): Promise<void> {
  try {
    if (choice === "system") window.localStorage?.removeItem(LOCALE_SETTING_KEY);
    else if ((AVAILABLE_LOCALES as readonly string[]).includes(choice)) window.localStorage?.setItem(LOCALE_SETTING_KEY, choice);
    else return;
  } catch {
    // Storage blocked: the switch still applies for this session.
  }
  await i18next.changeLanguage(detectLocale(choice === "system" ? null : choice));
  reportUiLocale();
}

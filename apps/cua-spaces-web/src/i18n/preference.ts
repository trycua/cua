// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The person's language choice: the Settings Language row and the welcome
 * screen's switcher. "system" follows the computer's languages.
 */

import { useSyncExternalStore } from "react";

import i18next, { changeLocale } from "./index";
import { hostLanguages } from "./host-languages";
import { LANGUAGES, LOCALE_STORAGE_KEY, detectLocale, systemLocale, type UiLanguage } from "./languages";

export type LocaleChoice = "system" | string;

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** The saved choice, or "system". */
export function savedLocaleChoice(): LocaleChoice {
  const saved = storage()?.getItem(LOCALE_STORAGE_KEY);
  return saved && LANGUAGES.some((l) => l.code === saved) ? saved : "system";
}

/** Saves the choice and switches the app now, without a restart. */
export async function setLocaleChoice(choice: LocaleChoice): Promise<void> {
  if (choice !== "system" && !LANGUAGES.some((l) => l.code === choice)) return;
  try {
    if (choice === "system") storage()?.removeItem(LOCALE_STORAGE_KEY);
    else storage()?.setItem(LOCALE_STORAGE_KEY, choice);
  } catch {
    // Storage blocked: the switch still applies for this session.
  }
  await changeLocale(detectLocale(choice === "system" ? null : choice, hostLanguages(), LANGUAGES));
}

const subscribe = (onChange: () => void) => {
  i18next.on("languageChanged", onChange);
  return () => i18next.off("languageChanged", onChange);
};

/** The switcher's state: the choice, the language in use, what "System"
 * means here, and the languages to offer. Re-renders on a switch. */
export function useLocaleChoice(): {
  choice: LocaleChoice;
  current: string;
  system: UiLanguage;
  languages: readonly UiLanguage[];
  setChoice: (choice: LocaleChoice) => Promise<void>;
} {
  const current = useSyncExternalStore(subscribe, () => i18next.language);
  const sys = systemLocale(hostLanguages(), LANGUAGES);
  return {
    choice: savedLocaleChoice(),
    current,
    system: LANGUAGES.find((l) => l.code === sys) ?? LANGUAGES[0]!,
    languages: LANGUAGES,
    setChoice: setLocaleChoice,
  };
}

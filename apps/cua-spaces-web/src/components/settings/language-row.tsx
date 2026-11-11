// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { SettingsRow } from "@/components/settings-group";
import { Select } from "@/components/ui/select";
import { toastError } from "@/components/ui/toast";
import { PSEUDO_LOCALE, useT } from "@/i18n";
import { useLocaleChoice } from "@/i18n/preference";

/**
 * Settings, General, Language: "System (日本語)" follows the computer;
 * each language is listed in its own name, and switching applies at once.
 * Machine translations not yet reviewed say "Preview". Shown once the app
 * has more than English, or while the pseudo language is on (`?lang=pseudo`).
 */
export function LanguageRow() {
  const t = useT();
  const { choice, current, system, languages, setChoice } = useLocaleChoice();
  const pseudo = current === PSEUDO_LOCALE;
  if (languages.length < 2 && !pseudo) return null;
  const options = [
    { value: "system", label: t("settings.general.language_system", { language: system.endonym }) },
    ...languages.map((l) => ({
      value: l.code,
      label: l.status === "draft" ? t("settings.general.language_preview", { language: l.endonym }) : l.endonym,
    })),
    ...(pseudo ? [{ value: PSEUDO_LOCALE, label: t("settings.general.language_pseudo") }] : []),
  ];
  return (
    <SettingsRow
      label={t("settings.general.language")}
      description={t("settings.general.language_help")}
      control={
        <Select
          aria-label={t("settings.general.language")}
          value={pseudo ? PSEUDO_LOCALE : choice}
          options={options}
          onValueChange={(v) => void setChoice(v).catch(toastError(t("settings.general.language_failed")))}
        />
      }
    />
  );
}

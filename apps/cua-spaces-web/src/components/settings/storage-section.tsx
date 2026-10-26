// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useExperiments, useStorageSettings, type SettingsRow, type SettingsSection } from "@/bridge";
import { CoreSettingsSection } from "@/components/settings/core-section";
import { plainStorage as plain } from "@/lib/settings-copy";

const plainRow = (row: SettingsRow): SettingsRow => ({
  ...row,
  label: plain(row.label),
  value: row.value ? plain(row.value) : row.value,
  options: row.options.map((o) => ({ ...o, label: plain(o.label) })),
});

/**
 * The Volume page's Storage: where Cua Volume keeps its files, the Finder
 * volume and the cache. It used to sit in Settings, General; where the files
 * live is a Volume setting, so it is on the Volume page now. Every row is
 * the app core's (`drive_settings::storage_section`), in plain words.
 */
export function StorageSection() {
  const { data: experiments } = useExperiments();
  // Only with the Cua Volume experiment on (`settings.withStorage`); off, nothing it set up is touched.
  if (!experiments?.experiments.cuaVolume) return null;
  return <StorageSectionBody />;
}

function StorageSectionBody() {
  const { data, press, choose, edit, save } = useStorageSettings();
  if (!data?.section) return null;
  const section: SettingsSection = { ...data.section, rows: data.section.rows.map(plainRow) };
  return (
    <CoreSettingsSection
      section={section}
      onHeaderButton={save}
      onPress={(r) => press(r.id)}
      onChoose={(r, option) => choose(r.id, option)}
      onEdit={(r, value) => edit(r.id, value)}
    />
  );
}

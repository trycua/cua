// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Plain words for Settings: the few labels the pages say in their own words.
 */

import { t } from "@/i18n";

/** Launch at login, General. */
export const launchLabel = () => t("settings.general.launch_label");
export const launchHelp = () => t("settings.general.launch_help");

/** The core's "Last check: 2 minutes ago" as "Last checked 2 minutes ago" (About). */
export const lastChecked = (line: string) =>
  /^Last check:\s*/.test(line) ? t("settings.about.last_checked", { when: line.replace(/^Last check:\s*/, "") }) : line;

/** "S3-compatible" is jargon: your own storage (S3). */
export const plainStorage = (text: string) => text.replace(/S3-compatible( bucket| storage)?/gi, () => t("settings.storage.own_storage"));

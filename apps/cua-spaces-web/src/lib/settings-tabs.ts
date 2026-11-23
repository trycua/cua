// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { t } from "@/i18n";

/**
 * Settings' tabs, in order. Account is the page at `/settings`. A label is
 * read when it is drawn (a getter), so it follows the language.
 */
export const SETTINGS_TABS = [
  {
    to: "/settings",
    get label() {
      return t("settings.nav.account");
    },
    exact: true,
  },
  {
    to: "/settings/general",
    get label() {
      return t("settings.nav.general");
    },
    exact: false,
  },
  {
    to: "/settings/connections",
    get label() {
      return t("settings.nav.connections");
    },
    exact: false,
  },
  {
    to: "/settings/advanced",
    get label() {
      return t("settings.nav.advanced");
    },
    exact: false,
  },
  {
    to: "/settings/about",
    get label() {
      return t("settings.nav.about");
    },
    exact: false,
  },
] as const;

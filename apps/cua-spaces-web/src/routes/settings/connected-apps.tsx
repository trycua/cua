// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { AccountKeysSection } from "@/components/settings/account-keys";
import { CliSettingsSection } from "@/components/settings/cli-section";
import { CodingAppsSection } from "@/components/settings/coding-apps-section";

export const Route = createFileRoute("/settings/connected-apps")({ component: ConnectedAppsPage });

/** Settings, Connected apps (was Agents): the coding apps Cua works with, the account keys they use, and Cua in Terminal. */
function ConnectedAppsPage() {
  return (
    <div data-settings-tab="connected-apps">
      <CodingAppsSection />
      <AccountKeysSection />
      <CliSettingsSection />
    </div>
  );
}

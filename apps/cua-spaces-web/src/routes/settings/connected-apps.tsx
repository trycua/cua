// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { useAgentKeys, useCliInstall, useSettings } from "@/bridge";
import { AccountKeysSection } from "@/components/settings/account-keys";
import { CliSettingsSection } from "@/components/settings/cli-section";
import { CodingAppsSection } from "@/components/settings/coding-apps-section";
import { useConnectedApps } from "@/hooks/use-connected-apps";
import { useQuiet } from "@/hooks/use-quiet";

export const Route = createFileRoute("/settings/connected-apps")({ component: ConnectedAppsPage });

/**
 * Settings, Connected apps (was Agents): the coding apps Cua works with, the account keys they use, and Cua in Terminal.
 * Its parts come from separate host reads; the page draws once they have answered, not one by one with the page jumping at each (CUA-1951).
 */
function ConnectedAppsPage() {
  const { data } = useSettings();
  const apps = useConnectedApps();
  const keys = useAgentKeys({ watch: true });
  const cli = useCliInstall();
  const settled = useQuiet(`${apps.rows?.length ?? -1}|${data?.cli ? 1 : 0}|${keys.view?.rows.length ?? 0}|${cli.state.plan ? 1 : 0}`);
  if (!settled) return <div className="h-96" aria-busy="true" />;
  return (
    <div data-settings-tab="connected-apps">
      <CodingAppsSection apps={apps} />
      <AccountKeysSection />
      <CliSettingsSection />
    </div>
  );
}

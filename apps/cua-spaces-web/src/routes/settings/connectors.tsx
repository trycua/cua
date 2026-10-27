// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { useAgentKeys, useCliInstall, useSettings } from "@/bridge";
import { AccountKeysSection } from "@/components/settings/account-keys";
import { CliSettingsSection } from "@/components/settings/cli-section";
import { CodingAppsSection } from "@/components/settings/coding-apps-section";
import { useConnectedApps } from "@/hooks/use-connected-apps";
import { useQuiet } from "@/hooks/use-quiet";

/** `app` is a coding app to bring into view (the Agents page's Connect). */
export const Route = createFileRoute("/settings/connectors")({
  component: ConnectorsPage,
  validateSearch: (s: Record<string, unknown>): { app?: string } => ({ app: typeof s.app === "string" ? s.app : undefined }),
});

/**
 * Settings, Connectors (was Connectors, Agents): the coding apps Cua works with, the account keys they use, and Cua in Terminal.
 * Its parts come from separate host reads; the page draws once they have answered, not one by one with the page jumping at each (CUA-1951).
 */
function ConnectorsPage() {
  const { app } = Route.useSearch();
  const { data } = useSettings();
  const apps = useConnectedApps();
  const keys = useAgentKeys({ watch: true });
  const cli = useCliInstall();
  const settled = useQuiet(`${apps.rows?.length ?? -1}|${data?.cli ? 1 : 0}|${keys.view?.rows.length ?? 0}|${cli.state.plan ? 1 : 0}`);
  if (!settled) return <div className="h-96" aria-busy="true" />;
  return (
    <div data-settings-tab="connectors">
      <CodingAppsSection apps={apps} focusApp={app} />
      <AccountKeysSection />
      <CliSettingsSection />
    </div>
  );
}

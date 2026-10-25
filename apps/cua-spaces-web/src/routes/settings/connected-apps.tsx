// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { useEffect } from "react";

import { useAgentKeys, useCliInstall, useSettings } from "@/bridge";
import { AccountKeysSection } from "@/components/settings/account-keys";
import { CliSettingsSection } from "@/components/settings/cli-section";
import { CodingAppsSection } from "@/components/settings/coding-apps-section";
import { useQuiet } from "@/hooks/use-quiet";

export const Route = createFileRoute("/settings/connected-apps")({ component: ConnectedAppsPage });

/**
 * Settings, Connected apps (was Agents): the coding apps Cua works with, the account keys they use, and Cua in Terminal.
 * Its parts come from separate host reads; the page draws once they have answered, not one by one with the page jumping at each (CUA-1951).
 */
function ConnectedAppsPage() {
  const { data, loadAgentRows } = useSettings();
  // The apps are read here, before the page draws, so their rows are not what arrives late.
  useEffect(() => {
    void loadAgentRows().catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per showing
  }, []);
  const keys = useAgentKeys({ watch: true });
  const cli = useCliInstall();
  const settled = useQuiet(`${data?.page?.sections.length ?? 0}|${data?.cli ? 1 : 0}|${keys.view?.rows.length ?? 0}|${cli.state.plan ? 1 : 0}|${data?.page?.sections.find((s) => s.id === "agents")?.rows.length ?? 0}`);
  if (!settled) return <div className="h-96" aria-busy="true" />;
  return (
    <div data-settings-tab="connected-apps">
      <CodingAppsSection />
      <AccountKeysSection />
      <CliSettingsSection />
    </div>
  );
}

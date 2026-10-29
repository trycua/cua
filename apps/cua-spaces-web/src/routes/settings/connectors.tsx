// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Outlet, useMatch } from "@tanstack/react-router";

import { useCliInstall, useSettings } from "@/bridge";
import { CliSettingsSection } from "@/components/settings/cli-section";
import { ConnectorsRedesign } from "@/components/settings/connectors/connectors-page";
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
  // An app's own page (`/settings/connectors/<app>`) draws in place of the list.
  const child = useMatch({ from: "/settings/connectors/$app", shouldThrow: false });
  if (child) return <Outlet />;
  return <Redesign />;
}

/** Coding apps with Connect and their pages, then Providers. */
function Redesign() {
  const { app } = Route.useSearch();
  const { data } = useSettings();
  const cli = useCliInstall();
  const settled = useQuiet(`${data?.cli ? 1 : 0}|${cli.state.plan ? 1 : 0}`);
  if (!settled) return <div className="h-96" aria-busy="true" />;
  return (
    <div data-settings-tab="connectors">
      <ConnectorsRedesign focusApp={app} />
      <CliSettingsSection />
    </div>
  );
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Outlet, useMatch } from "@tanstack/react-router";

import { useCliInstall, useSettings } from "@/bridge";
import { CliSettingsSection } from "@/components/settings/cli-section";
import { ConnectorsRedesign } from "@/components/settings/connections/connectors-page";
import { useQuiet } from "@/hooks/use-quiet";

/** `app` is a coding app to bring into view (the Agents page's Connect). */
export const Route = createFileRoute("/settings/connections")({
  component: ConnectorsPage,
  validateSearch: (s: Record<string, unknown>): { app?: string } => ({ app: typeof s.app === "string" ? s.app : undefined }),
});

/**
 * Settings, Connections (was Connectors, and Agents before): the providers connected once for every app that can use them, the coding apps Cua works with, and Cua in Terminal.
 * Its parts come from separate host reads; the page draws once they have answered, not one by one with the page jumping at each (CUA-1951).
 */
function ConnectorsPage() {
  // An app's own page (`/settings/connections/<app>`) draws in place of the list.
  const child = useMatch({ from: "/settings/connections/$app", shouldThrow: false });
  if (child) return <Outlet />;
  return <Redesign />;
}

/** The connections, then coding apps with Connect and their pages. */
function Redesign() {
  const { app } = Route.useSearch();
  const { data } = useSettings();
  const cli = useCliInstall();
  const settled = useQuiet(`${data?.cli ? 1 : 0}|${cli.state.plan ? 1 : 0}`);
  if (!settled) return <div className="h-96" aria-busy="true" />;
  return (
    <div data-settings-tab="connections">
      <ConnectorsRedesign focusApp={app} />
      <CliSettingsSection />
    </div>
  );
}

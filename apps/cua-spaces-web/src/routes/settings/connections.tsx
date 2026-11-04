// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Outlet, useMatch } from "@tanstack/react-router";

import { ConnectorsRedesign } from "@/components/settings/connections/connectors-page";

/** `app` is a coding app to bring into view (the Agents page's Connect). */
export const Route = createFileRoute("/settings/connections")({
  component: ConnectorsPage,
  validateSearch: (s: Record<string, unknown>): { app?: string } => ({ app: typeof s.app === "string" ? s.app : undefined }),
});

/**
 * Settings, Connections (was Connectors, and Agents before): the coding apps Cua works with, how to use this machine from another, and the providers connected once for every app that can use them.
 * (Cua in Terminal moved to General.) The page draws once the host has answered, not part by part with the page jumping (CUA-1951).
 */
function ConnectorsPage() {
  // An app's own page (`/settings/connections/<app>`) draws in place of the list.
  const child = useMatch({ from: "/settings/connections/$app", shouldThrow: false });
  if (child) return <Outlet />;
  return <Redesign />;
}

function Redesign() {
  const { app } = Route.useSearch();
  return (
    <div data-settings-tab="connections">
      <ConnectorsRedesign focusApp={app} />
    </div>
  );
}

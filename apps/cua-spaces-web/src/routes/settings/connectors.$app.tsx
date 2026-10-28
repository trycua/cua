// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { flagOn } from "@/bridge";
import { ConnectorAppPage } from "@/components/settings/connectors/app-detail";

/** One coding app's page under Connectors (the redesign; off, Connectors as it was). */
export const Route = createFileRoute("/settings/connectors/$app")({
  beforeLoad: ({ params }) => {
    if (!flagOn("spaces_redesign")) throw redirect({ to: "/settings/connectors", search: { app: params.app } });
  },
  component: AppPage,
});

function AppPage() {
  const { app } = Route.useParams();
  return <ConnectorAppPage agent={app} />;
}

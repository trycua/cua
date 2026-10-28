// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { ConnectorAppPage } from "@/components/settings/connectors/app-detail";

/** One coding app's page under Connectors. */
export const Route = createFileRoute("/settings/connectors/$app")({
  component: AppPage,
});

function AppPage() {
  const { app } = Route.useParams();
  return <ConnectorAppPage agent={app} />;
}

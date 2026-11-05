// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { DriverPage } from "@/components/driver/driver-page";

/**
 * Cua Driver (CUA-2706): this Mac's, or another computer's with
 * `?machine=<id>`. The page says so when this Cua Spaces carries no driver.
 */
export const Route = createFileRoute("/driver/")({
  validateSearch: (s: Record<string, unknown>): { machine?: string } => (typeof s.machine === "string" && s.machine ? { machine: s.machine } : {}),
  component: DriverRoute,
});

function DriverRoute() {
  const { machine } = Route.useSearch();
  return <DriverPage key={machine ?? ""} machine={machine ?? null} />;
}

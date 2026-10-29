// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { flagOn } from "@/bridge";
import { DriverPage } from "@/components/driver/driver-page";

/**
 * Cua Driver (CUA-2706): this Mac's, or another computer's with
 * `?machine=<id>`. Only while the `spaces_embedded_driver` flag is on;
 * Spaces otherwise.
 */
export const Route = createFileRoute("/driver/")({
  validateSearch: (s: Record<string, unknown>): { machine?: string } => (typeof s.machine === "string" && s.machine ? { machine: s.machine } : {}),
  beforeLoad: () => {
    if (!flagOn("spaces_embedded_driver")) throw redirect({ to: "/spaces" });
  },
  component: DriverRoute,
});

function DriverRoute() {
  const { machine } = Route.useSearch();
  return <DriverPage key={machine ?? ""} machine={machine ?? null} />;
}

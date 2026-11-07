// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { DriverPage } from "@/components/driver/driver-page";
import { THIS_COMPUTER } from "@/lib/machines";

/**
 * Cua Driver (CUA-2706), one of a computer's settings (CUA-3551): this Mac's
 * under `this-mac`, another computer's under its id. The page says so when
 * this Cua Spaces carries no driver.
 */
export const Route = createFileRoute("/spaces/computer/$computerId/driver")({
  component: DriverRoute,
});

function DriverRoute() {
  const { computerId } = Route.useParams();
  const machine = computerId === THIS_COMPUTER ? null : computerId;
  return <DriverPage key={computerId} machine={machine} />;
}

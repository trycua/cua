// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { ComputerPage } from "@/components/computers/computer-page";

/** One computer's page, opened from your list of computers; `this-mac` is this one (CUA-3551). */
export const Route = createFileRoute("/spaces/computer/$computerId/")({
  component: ComputerRoute,
});

function ComputerRoute() {
  const { computerId } = Route.useParams();
  return <ComputerPage key={computerId} id={computerId} />;
}

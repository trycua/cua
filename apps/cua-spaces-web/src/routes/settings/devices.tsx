// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { THIS_COMPUTER } from "@/lib/machines";

/**
 * The old Settings > Trusted machines tab. Its trust moved to this computer's page, so a
 * bookmark or a link to it lands there. Kept only for those old links.
 */
export const Route = createFileRoute("/settings/devices")({
  beforeLoad: () => {
    throw redirect({ to: "/spaces/computer/$computerId", params: { computerId: THIS_COMPUTER }, replace: true });
  },
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

/**
 * The old Settings > Trusted machines tab. Everything on it moved to the Machines tab, so a
 * bookmark or a link to it lands there. Kept only for those old links.
 */
export const Route = createFileRoute("/settings/devices")({
  beforeLoad: () => {
    throw redirect({ to: "/machines", replace: true });
  },
});

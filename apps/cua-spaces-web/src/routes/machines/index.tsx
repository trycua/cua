// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { machinesLinkTarget } from "@/lib/old-links";

/** The old Machines page: your computers are one list now (CUA-3551). Kept only so old links land there. */
export const Route = createFileRoute("/machines/")({
  validateSearch: (s: Record<string, unknown>) => s,
  beforeLoad: ({ search }) => {
    throw redirect({ ...machinesLinkTarget(search), replace: true });
  },
});

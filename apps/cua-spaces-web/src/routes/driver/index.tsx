// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { driverLinkTarget } from "@/lib/old-links";

/** The old Cua Driver page: it is one of a computer's settings now (CUA-3551). Kept only so old links land there. */
export const Route = createFileRoute("/driver/")({
  validateSearch: (s: Record<string, unknown>) => s,
  beforeLoad: ({ search }) => {
    throw redirect({ ...driverLinkTarget(search), replace: true });
  },
});

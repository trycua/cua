// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

/** Settings, Connected apps became Connectors. Kept for old links and builds (CUA-2381). */
export const Route = createFileRoute("/settings/connected-apps")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/connections", replace: true });
  },
});

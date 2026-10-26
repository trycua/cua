// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

/** Settings, Agents became Connected apps. */
export const Route = createFileRoute("/settings/agents")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/connected-apps", replace: true });
  },
});

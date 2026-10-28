// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

/** Settings, Experiments folded into Advanced. */
export const Route = createFileRoute("/settings/experiments")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/advanced", replace: true });
  },
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { flagOn } from "@/bridge";

export const Route = createFileRoute("/")({
  beforeLoad: () => {
    // The "Ask first" Home experiment opens on Home; otherwise Spaces, as before.
    throw redirect({ to: flagOn("spaces_ask_first_home") ? "/home" : "/spaces" });
  },
});

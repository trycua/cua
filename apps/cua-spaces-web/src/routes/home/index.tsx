// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { flagOn } from "@/bridge";
import { AskFirstHome } from "@/components/ask-first/home";

/** The "Ask first" Home, while its experiment flag (`spaces_ask_first_home`) is on; Spaces otherwise. */
export const Route = createFileRoute("/home/")({
  beforeLoad: () => {
    if (!flagOn("spaces_ask_first_home")) throw redirect({ to: "/spaces" });
  },
  component: AskFirstHome,
});

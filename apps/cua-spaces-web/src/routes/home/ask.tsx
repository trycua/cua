// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { AskFirstHome } from "@/components/ask-first/home";

/** The full composer: every agent, account and model, a Space or the agent's own Home. Run something's "More options". */
export const Route = createFileRoute("/home/ask")({
  component: AskFirstHome,
});

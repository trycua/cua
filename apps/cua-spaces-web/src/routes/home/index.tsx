// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { AskFirstHome } from "@/components/ask-first/home";

/** Home: asks what you want done and where. */
export const Route = createFileRoute("/home/")({
  component: AskFirstHome,
});

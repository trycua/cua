// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import type { KvCategory } from "@/bridge";
import { KeyvaultPage } from "@/components/keyvault/keyvault-page";

const CATEGORIES: readonly KvCategory[] = ["all", "waiting", "access", "recent"];

export const Route = createFileRoute("/keyvault/")({
  component: KeyvaultRoute,
  // A Space's "Signed in" badge opens Access with that Space's row brought forward.
  validateSearch: (s: Record<string, unknown>): { view?: KvCategory; focus?: string } => ({
    view: CATEGORIES.find((c) => c === s.view),
    focus: typeof s.focus === "string" ? s.focus : undefined,
  }),
});

function KeyvaultRoute() {
  const { view, focus } = Route.useSearch();
  return <KeyvaultPage initialView={view} focus={focus ?? null} />;
}

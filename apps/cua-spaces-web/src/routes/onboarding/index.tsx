// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

import { readSavedOnboarding, writeSavedOnboarding } from "@/bridge/onboarding";
import { NewFirstRun } from "@/components/first-run-new/first-run-new";

export const Route = createFileRoute("/onboarding/")({
  // `?due=1` is a host opening the first run because it is not done (the Mac
  // app at launch, or when its window opens again). The page keeps to what
  // the person chose: after "Skip for now" it goes to Spaces instead, as the
  // root route does for a host that opens the app at `/`. Every other way in
  // (the sidebar's "Finish setting up", Settings, the command palette) has
  // no `due` and always shows the first run.
  // `?fresh=1` is the Mac app's first launch on this computer: what the
  // page kept from an earlier install (a "Skip for now", a half-done run) is
  // forgotten, so a new user always sees the first run (CUA-3063).
  validateSearch: (s: Record<string, unknown>): { due?: true; fresh?: true } => {
    const on = (v: unknown) => v === 1 || v === "1" || v === true;
    return { ...(on(s.due) ? { due: true as const } : {}), ...(on(s.fresh) ? { fresh: true as const } : {}) };
  },
  beforeLoad: ({ search }) => {
    if (search.fresh) writeSavedOnboarding(null);
    else if (search.due && readSavedOnboarding().skipped) throw redirect({ to: "/spaces" });
  },
  component: NewFirstRun,
});

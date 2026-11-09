// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Skip for now" keeps the first run away: the Mac app opens `/onboarding?due=1`
// while the first run is not done (at launch, when its window opens again), and
// the page goes to Spaces if the person skipped it. Without `due` (the
// sidebar's "Finish setting up", Settings, the command palette) it always
// shows. The welcome used to come back at every launch, in Cua Spaces Local as
// in the released app, because the app opened `/onboarding` itself.

import { createMemoryHistory, createRouter } from "@tanstack/react-router";
import { beforeEach, describe, expect, it } from "vitest";

import { resetSavedOnboardingCache, writeSavedOnboarding } from "@/bridge/onboarding";
import { routeTree } from "../routeTree.gen";

/** A launch: the saved choice is read from storage again, as a new process does. */
async function landsAt(path: string): Promise<string> {
  resetSavedOnboardingCache();
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: [path] }) });
  await router.load();
  return router.state.location.pathname;
}

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
});

describe("the first run after Skip for now", () => {
  it("opens on a launch while it is not done and was not skipped", async () => {
    expect(await landsAt("/onboarding?due=1")).toBe("/onboarding");
  });

  it("stays away on a launch after Skip for now, and stays away at the next one", async () => {
    writeSavedOnboarding({ state: null, skipped: true });
    expect(await landsAt("/onboarding?due=1")).toBe("/spaces");
    expect(await landsAt("/onboarding?due=1")).toBe("/spaces");
  });

  it("still shows when a person asks for it, skipped or not", async () => {
    writeSavedOnboarding({ state: null, skipped: true });
    expect(await landsAt("/onboarding")).toBe("/onboarding");
    expect(await landsAt("/onboarding?due=0")).toBe("/onboarding");
  });
});

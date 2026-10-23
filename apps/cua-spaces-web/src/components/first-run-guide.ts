// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The guided first run on the Spaces page (CUA-2062): "Create your first
 * Space" on the empty home, then "Your first Space is ready" with one task to
 * try. Each step is counted once per install, as a feature event:
 * `first_run_guide_shown`, `first_space_created`, `first_space_ready`,
 * `suggested_task_started`. Created and ready only count on an install that
 * showed the guide, so people who already had Spaces are not counted again.
 */

import { useEffect } from "react";

import { useFeatureRecorder, type Space } from "@/bridge";
import { spaceState } from "@/lib/spaces";

const KEY = "cua.firstRun";

export type FirstRunStep = "guide_shown" | "space_created" | "space_ready" | "task_started";

const EVENT: Record<FirstRunStep, string> = {
  guide_shown: "first_run_guide_shown",
  space_created: "first_space_created",
  space_ready: "first_space_ready",
  task_started: "suggested_task_started",
};

function seen(step: FirstRunStep): boolean {
  try {
    return globalThis.localStorage?.getItem(`${KEY}.${step}`) === "1";
  } catch {
    return false;
  }
}

/** Marks a step done; true the first time only. */
function mark(step: FirstRunStep): boolean {
  if (seen(step)) return false;
  try {
    globalThis.localStorage?.setItem(`${KEY}.${step}`, "1");
  } catch {
    // Not kept: it may count again on the next launch.
  }
  return true;
}

/** Records a first-run step once per install. */
export function useFirstRunStep(): (step: FirstRunStep) => void {
  const record = useFeatureRecorder();
  return (step) => {
    if (mark(step)) record(EVENT[step]);
  };
}

/** Counts the first Space's create and its ready, after the guide was shown. */
export function useFirstSpaceMilestones(spaces: Space[]): void {
  const step = useFirstRunStep();
  const creating = spaces.some((s) => spaceState(s) === "creating");
  const running = spaces.some((s) => spaceState(s) === "running");
  useEffect(() => {
    if (!seen("guide_shown")) return;
    if (creating || running) step("space_created");
    if (running) step("space_ready");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [creating, running]);
}

/** The one task offered once the first Space is ready: real work where a clean computer matters. */
export interface SuggestedTask {
  title: string;
  why: string;
  prompt: string;
}

export function suggestedTask(space: Space): SuggestedTask {
  const system = space.os === "macos" ? "Mac" : "Linux computer";
  return {
    title: `Run a project's tests on a clean ${system}`,
    why: `See if your code works on a computer that has never seen it, without touching your own setup.`,
    prompt: `In the Cua Space "${space.name}", clone <link to your GitHub repo>, install what it needs from its README, and run its tests. Tell me which tests fail on this clean computer and why.`,
  };
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings, About, "Reset & Show": the welcome forgets what it kept and
 * starts again at its first step.
 *
 * What it forgets is the app core's list (`onboarding.resetPlan`,
 * `cua-spaces-app-core/src/onboarding_reset.rs`), the same for every host:
 * the page's own storage here (`webKeys`), and the host's `onboarding.json`
 * (`hostFiles`, which the host removes on `session.resetOnboarding`).
 * Nothing else is touched: no Space, computer, Volume, Keyvault item,
 * sign-in, cloud, experiment or other setting, and the coding apps keep
 * their Cua config (only the record of the choice is cleared).
 *
 * Safe to run twice: removing what is not there is no change.
 */

import { resetConnectedAppsState } from "@/hooks/use-connected-apps";

import type { CoreClient } from "./core";
import { writeFirstRun } from "./first-run";
import { writeSavedOnboarding } from "./onboarding";

/** `onboarding_reset::OnboardingResetPlan`. */
export interface OnboardingResetPlan {
  webKeys: string[];
  hostFiles: string[];
  firstStep: string;
}

/** The plan without the core (a build with no wasm): the same lists. A test keeps them equal to the core's. */
export const FALLBACK_RESET_PLAN: OnboardingResetPlan = {
  webKeys: ["cua-spaces:onboarding", "cua-spaces:first-run", "cua.upsell.dismissed.onboarding", "cua.firstRun.teammates", "cua.connectedApps"],
  hostFiles: ["onboarding.json"],
  firstStep: "welcome",
};

export function onboardingResetPlan(core: CoreClient): OnboardingResetPlan {
  return core.tryCall<OnboardingResetPlan>("onboarding.resetPlan") ?? FALLBACK_RESET_PLAN;
}

/**
 * Clears the page's welcome state: exactly the plan's keys, then the copies
 * the page holds in memory, so a page that is open starts at the first step
 * at once.
 */
export function forgetWelcome(core: CoreClient): OnboardingResetPlan {
  const plan = onboardingResetPlan(core);
  try {
    for (const key of plan.webKeys) globalThis.localStorage?.removeItem(key);
  } catch {
    // No storage (private mode): there is nothing kept to clear.
  }
  writeSavedOnboarding(null);
  writeFirstRun(null);
  resetConnectedAppsState();
  return plan;
}

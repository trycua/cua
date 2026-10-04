// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Which anonymous usage events a UI step means, from the app core
 * (`telemetry::*`): the first run's pages, Space creates, Settings, Storage,
 * the Share sheet and enrolling this device. The shell sends the signals
 * (`telemetry_record_signals`); the SwiftUI app derives and sends the same
 * ones through the cua SDK's bindings, so one flow means the same events in
 * either app (the `telemetry-funnel` parity flow).
 */
import { core } from "../core";
import type { CreateAction, CreatesState } from "./creates";
import type { StorageAction, StorageInput, StorageState } from "./driveSettings";
import type { EnrollAction, EnrollState } from "./devices";
import type { OnboardingAction, OnboardingState } from "./onboarding";
import type { ShareInput, ShareSheetAction, ShareSheetState } from "./share";

/** One usage event (fixed words, flags and a duration; never names). */
export type TelemetrySignal =
  | { type: "feature"; feature: string }
  | { type: "step"; step: string; ok: boolean }
  | { type: "onboarding-page"; page: string; action: string; choice: string }
  | { type: "space-wizard"; action: "opened" | "cancelled" | "submitted" }
  | {
      type: "space-create";
      location: string;
      guestOs: string;
      kind: string;
      outcome: string;
      failedPhase: string;
      stalled: boolean;
      elapsedMs: number;
      gpu: boolean;
    }
  | { type: "space-create-started"; location: string; guestOs: string; kind: string; gpu: boolean }
  | {
      type: "volume-setup";
      surface: string;
      storage: string;
      addToFinder: boolean;
      mountMethod: string;
      outcome: string;
    }
  | { type: "share"; action: string; role: string; outcome: string }
  | { type: "app-update"; action: string; channel: string; trigger: string }
  | { type: "device-enroll"; method: string; outcome: string }
  | { type: "experiment"; action: "experiment_on" | "experiment_off"; experiment: string }
  | { type: "experiments-on"; experiments: string[] };

/** A fixed feature name as a signal. */
export function featureSignals(feature: string): TelemetrySignal[] {
  return core("telemetry.feature", { feature });
}

/** One first-run step (the state before it). */
export function onboardingSignals(state: OnboardingState, action: OnboardingAction): TelemetrySignal[] {
  return core("telemetry.onboarding", { state, action });
}

/** "Start using Cua Spaces". */
export function onboardingFinishedSignals(state: OnboardingState): TelemetrySignal[] {
  return core("telemetry.onboardingFinished", { state });
}

/** One step of the Spaces being created, at `now` (the state before it). */
export function createsSignals(state: CreatesState | null, action: CreateAction, now: number): TelemetrySignal[] {
  return core("telemetry.creates", { state, action, now });
}

/** One step of Settings, Storage (the state before it). */
export function storageSignals(input: StorageInput, state: StorageState, action: StorageAction): TelemetrySignal[] {
  return core("telemetry.storage", { input, state, action });
}

/** One step of the Share sheet (the state before it). */
export function shareSignals(input: ShareInput, state: ShareSheetState, action: ShareSheetAction): TelemetrySignal[] {
  return core("telemetry.share", { input, state, action });
}

/** One step of the enroll sheet (the state before it). */
export function enrollSignals(state: EnrollState, action: EnrollAction): TelemetrySignal[] {
  return core("telemetry.enroll", { state, action });
}

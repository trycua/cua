// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings, Experiments, from the app core (`experiments::*`): one switch
 * per feature still being built, all off by default, and what each hides
 * while off (Cua Volume: the first run's Volume page and Settings, Storage;
 * Your cloud: your clouds in New Space; Sharing: a Space's Share button).
 * Off only hides: nothing set up is undone. The SwiftUI app draws the same.
 */
import { core } from "../core";
import type { TelemetrySignal } from "./telemetry";
import type { SettingsPage, SettingsSection } from "./window";

/** The switches, as stored (`cua.settings.experiments`). */
export interface Experiments {
  cuaVolume: boolean;
  yourCloud: boolean;
  sharing: boolean;
}

export const NO_EXPERIMENTS: Experiments = { cuaVolume: false, yourCloud: false, sharing: false };

/** The Experiments tab: a switch and one line per experiment. */
export function experimentsPage(experiments: Experiments): SettingsPage {
  return core("experiments.page", { experiments });
}

/** The switches after a row's choice (`on` or `off`). */
export function chooseExperiment(experiments: Experiments, row: string, option: string): Experiments {
  return core("experiments.choose", { experiments, row, option });
}

/** The Settings page with Storage after General, only with Cua Volume on. */
export function settingsWithStorage(
  page: SettingsPage,
  storage: SettingsSection,
  experiments: Experiments,
): SettingsPage {
  return core("settings.withStorage", { page, storage, experiments });
}

/** The events of a change (`experiment_on` / `experiment_off`, then the set). */
export function experimentsChangedSignals(before: Experiments, after: Experiments): TelemetrySignal[] {
  return core("telemetry.experimentsChanged", { before, after });
}

/** Which experiments are on (at launch). */
export function experimentsOnSignals(experiments: Experiments): TelemetrySignal[] {
  return core("telemetry.experimentsOn", { experiments });
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useSyncExternalStore } from "react";

import {
  chooseExperiment,
  experimentsChangedSignals,
  experimentsOnSignals,
  NO_EXPERIMENTS,
  type Experiments,
} from "../model/experiments";
import { telemetryBridge, type TelemetryBridge } from "../native/telemetry";
import { readSetting, SETTINGS_KEYS, writeJson } from "./settings";

/**
 * Settings, Experiments as this window holds them: read from the settings
 * storage (`cua.settings.experiments`; a missing or unknown key is off),
 * changed through the app core's `experiments.choose`, saved, and shared
 * with every component of the window that reads them.
 */

let cache: { raw: string; value: Experiments } | null = null;
const listeners = new Set<() => void>();

/** The switches now (the same object until the stored value changes). */
export function currentExperiments(): Experiments {
  const raw = readSetting(SETTINGS_KEYS.experiments, "");
  if (cache && cache.raw === raw) return cache.value;
  let stored: Partial<Record<keyof Experiments, unknown>> = {};
  try {
    stored = raw ? (JSON.parse(raw) as typeof stored) : {};
  } catch {
    stored = {};
  }
  const value: Experiments = {
    cuaVolume: stored.cuaVolume === true,
    yourCloud: stored.yourCloud === true,
    sharing: stored.sharing === true,
  };
  cache = { raw, value: { ...NO_EXPERIMENTS, ...value } };
  return cache.value;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** A row of the Experiments tab changed (`on` or `off`): saves the switches
 * and records the change (the events the core derives). */
export function chooseExperimentRow(
  row: string,
  option: string,
  telemetry: TelemetryBridge = telemetryBridge(),
): Experiments {
  const before = currentExperiments();
  const after = chooseExperiment(before, row, option);
  const signals = experimentsChangedSignals(before, after);
  if (signals.length === 0) return before;
  writeJson(SETTINGS_KEYS.experiments, after);
  telemetry.recordSignals(signals);
  for (const l of listeners) l();
  return currentExperiments();
}

/** At launch: tells telemetry which experiments are on (the day's
 * `cua_app_active` carries them). */
export function declareExperiments(telemetry: TelemetryBridge = telemetryBridge()): void {
  telemetry.recordSignals(experimentsOnSignals(currentExperiments()));
}

/** The switches, re-rendering when one changes. */
export function useExperiments(): Experiments {
  return useSyncExternalStore(subscribe, currentExperiments, currentExperiments);
}

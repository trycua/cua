// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `telemetry.track`: usage events from the web UI, on the host's existing
 * telemetry. The events are the app core's `TelemetrySignal`s
 * (`telemetry.rs`), derived from the same reducer steps the SwiftUI and
 * Tauri apps derive them from, so one run yields the same events in every
 * shell (the `telemetry-funnel` parity flow).
 *
 * - Tauri: `telemetry_record_signals` (the app's `cua-telemetry` client).
 * - SwiftUI: `telemetry.track`, which checks the switch, then records
 *   through `TelemetryRunning.record` (`appTelemetryRecord`).
 * - Electron: the same method, recorded the same way.
 */

import type { OpCoverage } from "../coverage";

/** One usage event (`telemetry::TelemetrySignal`): fixed words, flags and
 * durations, never names, paths, URLs, emails or anything typed. */
export type TelemetrySignal =
  | { type: "feature"; feature: string }
  | { type: "step"; step: string; ok: boolean }
  | { type: "launched"; onboardingEligible: boolean | null }
  | { type: "sign-in-failed"; errorKind: string }
  | { type: "onboarding-page"; page: string; action: string; choice: string }
  | { type: "space-wizard"; action: string }
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
      errorVariant: string;
    }
  | { type: "space-create-started"; location: string; guestOs: string; kind: string; gpu: boolean }
  | { type: "volume-setup"; surface: string; storage: string; addToFinder: boolean; mountMethod: string; outcome: string }
  | { type: "share"; action: string; role: string; outcome: string }
  | { type: "app-update"; action: string; channel: string; trigger: string }
  | { type: "device-enroll"; method: string; outcome: string }
  | { type: "experiment"; action: string; experiment: string }
  | { type: "experiments-on"; experiments: string[] };

export interface TelemetryOps {
  /** Records the signals on the host's telemetry, which drops them while
   * usage data is off or before the first-run notice. */
  "telemetry.track": { args: { signals: TelemetrySignal[] }; result: null };
}


/* ---- What may cross the bridge ------------------------------------------------ */

type Field = "word" | "flag" | "maybe-flag" | "ms" | "words" | "variant";

/** Every signal's fields and what each may hold. */
const SHAPES: Record<TelemetrySignal["type"], Record<string, Field>> = {
  feature: { feature: "word" },
  step: { step: "word", ok: "flag" },
  launched: { onboardingEligible: "maybe-flag" },
  "sign-in-failed": { errorKind: "word" },
  "onboarding-page": { page: "word", action: "word", choice: "word" },
  "space-wizard": { action: "word" },
  "space-create": {
    location: "word",
    guestOs: "word",
    kind: "word",
    outcome: "word",
    failedPhase: "word",
    stalled: "flag",
    elapsedMs: "ms",
    gpu: "flag",
    errorVariant: "variant",
  },
  "space-create-started": { location: "word", guestOs: "word", kind: "word", gpu: "flag" },
  "volume-setup": { surface: "word", storage: "word", addToFinder: "flag", mountMethod: "word", outcome: "word" },
  share: { action: "word", role: "word", outcome: "word" },
  "app-update": { action: "word", channel: "word", trigger: "word" },
  "device-enroll": { method: "word", outcome: "word" },
  experiment: { action: "word", experiment: "word" },
  "experiments-on": { experiments: "words" },
};

/** A fixed word from the schema (`space_create_local`, `this_mac`). Names,
 * emails, paths and URLs never match. */
const WORD = /^[a-z0-9_]{1,64}$/;
/** An error enum case (`InsufficientDisk`), or empty when the shell has none. */
const VARIANT = /^[A-Za-z0-9_]{0,64}$/;

function valid(kind: Field, v: unknown): boolean {
  switch (kind) {
    case "word":
      return typeof v === "string" && WORD.test(v);
    case "flag":
      return typeof v === "boolean";
    case "maybe-flag":
      return typeof v === "boolean" || v === null;
    case "ms":
      return typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
    case "words":
      return Array.isArray(v) && v.length <= 16 && v.every((w) => typeof w === "string" && WORD.test(w));
    case "variant":
      return typeof v === "string" && VARIANT.test(v);
  }
}

/**
 * The signals as they may leave the page: known types with exactly their
 * fields, each a fixed word, flag or duration. Anything else is dropped
 * whole, so a bug upstream can't send a name or a path.
 */
export function sanitizeSignals(signals: readonly unknown[]): TelemetrySignal[] {
  const out: TelemetrySignal[] = [];
  for (const s of signals) {
    if (!s || typeof s !== "object") continue;
    const type = (s as { type?: unknown }).type;
    const shape = typeof type === "string" ? SHAPES[type as TelemetrySignal["type"]] : undefined;
    if (!shape) continue;
    const clean: Record<string, unknown> = { type };
    let ok = true;
    for (const [key, kind] of Object.entries(shape)) {
      let v = (s as Record<string, unknown>)[key];
      if (kind === "variant" && v === undefined) v = "";
      if (!valid(kind, v)) {
        ok = false;
        break;
      }
      clean[key] = Array.isArray(v) ? [...v] : v;
    }
    if (ok) out.push(clean as TelemetrySignal);
  }
  return out;
}

/* ---- Hosts ------------------------------------------------------------------ */

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

export function tauriTelemetryOps(invoke: Invoke) {
  return {
    "telemetry.track": async ({ signals }: TelemetryOps["telemetry.track"]["args"]) => {
      await invoke("telemetry_record_signals", { signals });
      return null;
    },
  };
}

export const TELEMETRY_COVERAGE = {
  "telemetry.track": {
    webkit: { methods: ["telemetry.track"] },
    tauri: ["telemetry_record_signals"],
  },
} as const satisfies Record<keyof TelemetryOps, OpCoverage>;

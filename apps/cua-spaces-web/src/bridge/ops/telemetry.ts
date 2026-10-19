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
 *
 * `telemetry.linkNotice` and `telemetry.acknowledgeLinkNotice`: the one-time
 * notice for machines that saw the older "anonymous" wording (CUA-1881).
 * Usage data is linked to the signed-in account through a random ID, and
 * nothing links before this notice is closed. The host owns whether it is
 * due and the words (`cua_telemetry::link`), so every shell says the same.
 *
 * - Tauri: `telemetry_link_notice`, `telemetry_acknowledge_link_notice`.
 * - SwiftUI and Electron: the same method names, answered from the SDK's
 *   `telemetryLinkNotice`, `telemetryLinkNoticeDue` and
 *   `telemetryAcknowledgeLinkNotice`.
 * - Demo: no notice (`null`).
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

/** The one-time linking notice: the host's words, and whether it is due now. */
export interface TelemetryLinkNotice {
  /** Show it now: usage data is on, the account offered linking, and this
   * machine has not shown it. */
  due: boolean;
  title: string;
  body: string;
  /** "Got it": keeps usage data on. */
  okLabel: string;
  /** "Turn off usage data". */
  turnOffLabel: string;
  /** The link to what is collected. */
  detailsLabel: string;
  docsUrl: string;
}

export interface TelemetryOps {
  /** Records the signals on the host's telemetry, which drops them while
   * usage data is off or before the first-run notice. */
  "telemetry.track": { args: { signals: TelemetrySignal[] }; result: null };
  /** The one-time linking notice, or null when the host has none. */
  "telemetry.linkNotice": { args: Record<string, never>; result: TelemetryLinkNotice | null };
  /** The notice was closed (either button): linking may start at the next launch. */
  "telemetry.acknowledgeLinkNotice": { args: Record<string, never>; result: null };
}

export const TELEMETRY_OPERATIONS = [
  "telemetry.track",
  "telemetry.linkNotice",
  "telemetry.acknowledgeLinkNotice",
] as const satisfies readonly (keyof TelemetryOps)[];

const text = (v: unknown): v is string => typeof v === "string" && v.length > 0;

/** A host's answer as a notice; anything malformed reads as none. */
export function linkNoticeFromHost(raw: unknown): TelemetryLinkNotice | null {
  const n = raw as Partial<TelemetryLinkNotice> | null | undefined;
  if (!n || typeof n !== "object") return null;
  if (!text(n.title) || !text(n.body) || !text(n.okLabel) || !text(n.turnOffLabel) || !text(n.detailsLabel) || !text(n.docsUrl)) return null;
  return {
    due: n.due === true,
    title: n.title,
    body: n.body,
    okLabel: n.okLabel,
    turnOffLabel: n.turnOffLabel,
    detailsLabel: n.detailsLabel,
    docsUrl: n.docsUrl,
  };
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
      const v = (s as Record<string, unknown>)[key];
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
    "telemetry.linkNotice": async () => linkNoticeFromHost(await invoke("telemetry_link_notice")),
    "telemetry.acknowledgeLinkNotice": async () => {
      await invoke("telemetry_acknowledge_link_notice");
      return null;
    },
  };
}

type WebkitRequest = <T>(
  method: "telemetry.linkNotice" | "telemetry.acknowledgeLinkNotice",
  args?: Record<string, unknown>,
  wait?: number | null,
) => Promise<T>;

/** SwiftUI and Electron: both methods go to the host. An older build
 * (`unimplemented`) or any failure reads as no notice, so nothing links. */
export function webkitTelemetryNoticeOps(request: WebkitRequest) {
  return {
    "telemetry.linkNotice": () => request<unknown>("telemetry.linkNotice", {}, 5_000).then(linkNoticeFromHost, () => null),
    "telemetry.acknowledgeLinkNotice": async () => {
      await request("telemetry.acknowledgeLinkNotice", {});
      return null;
    },
  };
}

export const TELEMETRY_COVERAGE = {
  "telemetry.track": {
    webkit: { methods: ["telemetry.track"] },
    tauri: ["telemetry_record_signals"],
  },
  "telemetry.linkNotice": {
    webkit: { methods: ["telemetry.linkNotice"] },
    tauri: ["telemetry_link_notice"],
  },
  "telemetry.acknowledgeLinkNotice": {
    webkit: { methods: ["telemetry.acknowledgeLinkNotice"] },
    tauri: ["telemetry_acknowledge_link_notice"],
  },
} as const satisfies Record<keyof TelemetryOps, OpCoverage>;

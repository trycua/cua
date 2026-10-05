// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { TelemetrySignal } from "../model/telemetry";
import { hasTauri } from "./bridge";

/**
 * Anonymous usage telemetry (the shell's `telemetry_*` commands in
 * `src-tauri/src/telemetry.rs`, on the SDK's cua-telemetry core). The switch
 * is the same `$CUA_HOME/config.toml` setting as `cua telemetry off`.
 */

export interface TelemetryView {
  enabled: boolean;
  /** Why (`env DO_NOT_TRACK`, `config <path>`, `default`, ...). */
  source: string;
  /** `do_not_track`, `env`, `legacy_env`, `config`, `ci` or `default`. */
  sourceKind: string;
  noticeShown: boolean;
  noticeText: string;
  docsUrl: string;
}

export const TELEMETRY_DOCS_URL = "https://cua.ai/docs/cua-sdk/concepts/telemetry";

export interface TelemetryBridge {
  isNative: boolean;
  status(): Promise<TelemetryView>;
  setEnabled(enabled: boolean): Promise<TelemetryView>;
  acknowledgeNotice(): Promise<TelemetryView>;
  /**
   * The first run left Welcome with its usage-data switch at `on`: writes
   * the machine's setting when it changed, then records that the notice was
   * shown. Nothing is sent before; with it off, nothing after either.
   */
  welcomeLeft(on: boolean): Promise<TelemetryView>;
  /** A fixed feature name (see the docs); anything else is dropped. */
  recordFeature(feature: string): void;
  /** A fixed funnel step name; anything else is dropped. */
  recordStep(step: string, ok?: boolean): void;
  /** A media session ended; the shell buckets it into `cua_stream_stats`. */
  recordStream(stream: StreamReport): void;
  /** Signals the app core derived (`model/telemetry.ts`); fixed words only. */
  recordSignals(signals: TelemetrySignal[]): void;
}

/** What a viewer counted over one media session (raw; bucketed natively). */
export interface StreamReport {
  codec: string;
  frames: number;
  height: number;
  durationMs: number;
}

export function createTauriTelemetryBridge(): TelemetryBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    status: () => invoke<TelemetryView>("telemetry_status"),
    setEnabled: (enabled) => invoke<TelemetryView>("telemetry_set_enabled", { enabled }),
    acknowledgeNotice: () => invoke<TelemetryView>("telemetry_acknowledge_notice"),
    welcomeLeft: (on) => invoke<TelemetryView>("telemetry_welcome_left", { on }),
    recordFeature: (feature) => {
      void invoke<void>("telemetry_record_feature", { featureName: feature }).catch(() => {});
    },
    recordStep: (step, ok = true) => {
      void invoke<void>("telemetry_record_step", { stepName: step, ok }).catch(() => {});
    },
    recordSignals: (signals) => {
      if (signals.length === 0) return;
      void invoke<void>("telemetry_record_signals", { signals }).catch(() => {});
    },
    recordStream: (stream) => {
      void invoke<void>("telemetry_record_stream", {
        codec: stream.codec,
        frames: Math.max(0, Math.round(stream.frames)),
        height: Math.max(0, Math.round(stream.height)),
        durationMs: Math.max(0, Math.round(stream.durationMs)),
      }).catch(() => {});
    },
  };
}

/** Outside the app (browser dev, tests): nothing is recorded or sent. */
export function createFallbackTelemetryBridge(): TelemetryBridge {
  const off = (): TelemetryView => ({
    enabled: false,
    source: "not available outside the Cua Spaces app",
    sourceKind: "default",
    noticeShown: true,
    noticeText: "",
    docsUrl: TELEMETRY_DOCS_URL,
  });
  return {
    isNative: false,
    status: async () => off(),
    setEnabled: async () => off(),
    acknowledgeNotice: async () => off(),
    welcomeLeft: async () => off(),
    recordFeature: () => {},
    recordStep: () => {},
    recordStream: () => {},
    recordSignals: () => {},
  };
}

let shared: TelemetryBridge | null = null;

/** The bridge for this environment. */
export function telemetryBridge(): TelemetryBridge {
  shared ??= hasTauri() ? createTauriTelemetryBridge() : createFallbackTelemetryBridge();
  return shared;
}

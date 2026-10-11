// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's telemetry: the events it was sent, kept in memory, and
 * only while its usage-data setting is on (as the SDK's client drops them). */

import type { TelemetrySignal } from "../../ops/telemetry";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoTelemetryState {
  /** Every event recorded, in order. */
  telemetry: TelemetrySignal[];
}

export const demoTelemetryState = (): DemoTelemetryState => ({ telemetry: [] });

export function demoTelemetryHandlers({ state }: DemoContext): DemoHandlers<"telemetry.track"> {
  return {
    "telemetry.track": ({ signals }) => {
      if (state.settings.telemetry) state.telemetry.push(...signals);
      return null;
    },
  };
}

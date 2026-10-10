// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Usage events from the web UI. The app core decides which events a step
 * means (`telemetry.*`, the same calls the SwiftUI and Tauri apps make);
 * `TelemetryForwarder` sends them to the host with `telemetry.track`
 * (`ops/telemetry.ts`).
 *
 * The opt-out is the machine's own setting (`settings.get`'s `telemetry`,
 * the same switch as Settings and `cua telemetry off`): nothing is sent
 * unless the host reports it on and its first-run notice shown, read again
 * before every send. The host's telemetry checks the same switch again.
 * Only `sanitizeSignals`' fixed words, flags and durations leave the page.
 */

import { isUnsupported, type BridgeMode, type DataAdapter } from "./adapter";
import type { CoreClient } from "./core";
import type { TelemetryView } from "./contracts/host";
import type { OnboardingAction, OnboardingFlowState } from "./contracts/onboarding";
import type { CreateAction, CreatesState } from "./contracts/spaces";
import { sanitizeSignals, type TelemetrySignal } from "./ops/telemetry";

export type { TelemetrySignal } from "./ops/telemetry";

/* ---- The core's decisions ------------------------------------------------------ */

const signals = (v: TelemetrySignal[] | undefined): TelemetrySignal[] => v ?? [];

/** `app_launched` (`telemetry.launched`), with whether the first run is
 * still to finish (null: unknown). */
export function telemetryLaunched(core: CoreClient, onboardingEligible: boolean | null = null): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.launched", { onboardingEligible }));
}

/** What a first-run step means, from the state before it (`telemetry.onboarding`). */
export function telemetryOnboarding(core: CoreClient, state: OnboardingFlowState, action: OnboardingAction): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.onboarding", { state, action }));
}

/** The first run ended (`telemetry.onboardingFinished`). */
export function telemetryOnboardingFinished(core: CoreClient, state: OnboardingFlowState): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.onboardingFinished", { state }));
}

/** "Set up later" (`telemetry.onboardingSkipped`): the page it was left on. */
export function telemetryOnboardingSkipped(core: CoreClient, state: OnboardingFlowState): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.onboardingSkipped", { state }));
}

/** A sign-in that failed, timed out (`message`) or was cancelled (null),
 * as its kind (`telemetry.signInFailed`); the message never leaves the core. */
export function telemetrySignInFailed(core: CoreClient, message: string | null): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.signInFailed", { message }));
}

/** What a create, power or delete step means (`telemetry.creates`). */
export function telemetryCreates(core: CoreClient, state: CreatesState | null, action: CreateAction, now: number): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.creates", { state, action, now }));
}

/** Settings, Storage (`telemetry.storage`), for the screen that drives `storage.reduce`. */
export function telemetryStorage(core: CoreClient, input: unknown, state: unknown, action: unknown): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.storage", { input, state, action }));
}

/** The share sheet (`telemetry.share`), for the screen that drives `share.reduce`. */
export function telemetryShare(core: CoreClient, input: unknown, state: unknown, action: unknown): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.share", { input, state, action }));
}

/** Enrolling this device (`telemetry.enroll`), for the Devices screen. */
export function telemetryEnroll(core: CoreClient, state: unknown, action: unknown): TelemetrySignal[] {
  return signals(core.tryCall("telemetry.enroll", { state, action }));
}

/* ---- Sending ---------------------------------------------------------------------- */

/** Hosts that record `app_launched` themselves when they start (Tauri's
 * `telemetry::init`, the SwiftUI app's `appTelemetryStart`); the page
 * records it only where nothing else does. */
export const HOST_RECORDS_LAUNCH: Record<BridgeMode, boolean> = { tauri: true, webkit: true, electron: false, demo: false };

/** Usage data may be sent: the switch is on and the notice was shown. */
export function telemetryAllowed(view: TelemetryView | null | undefined): boolean {
  return view?.enabled === true && view.noticeShown !== false;
}

/**
 * Sends signals to the host in order, each batch only while the machine's
 * setting allows it at that moment. Never fails the caller: usage data is
 * not worth an error on screen.
 */
export class TelemetryForwarder {
  private queue: Promise<void> = Promise.resolve();
  /** The host has no telemetry route: stop asking. */
  private unrouted = false;

  constructor(
    private readonly adapter: DataAdapter,
    /** The machine's telemetry setting now (loads it when not known yet). */
    private readonly view: () => Promise<TelemetryView | null>,
  ) {}

  track(batch: readonly unknown[]): Promise<void> {
    const clean = sanitizeSignals(batch);
    if (clean.length === 0 || this.unrouted) return this.queue;
    this.queue = this.queue.then(async () => {
      if (this.unrouted) return;
      const view = await this.view().catch(() => null);
      if (!telemetryAllowed(view)) return;
      try {
        await this.adapter.call("telemetry.track", { signals: clean });
      } catch (e) {
        if (isUnsupported(e)) this.unrouted = true;
      }
    });
    return this.queue;
  }

  /** Resolves once everything tracked so far was sent or dropped. */
  flush(): Promise<void> {
    return this.queue;
  }
}

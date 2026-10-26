// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Usage events from the page (`telemetry.track {signals}`): the core's
// fixed words, flags and durations, checked word by word, then recorded
// through the app's switch (the SwiftUI host's `telemetrySignal`). Off, or
// locked off, drops every signal here.
import type { Native } from "../native/load";
import type { AppTelemetrySignal } from "../native/generated/index";
import { crashReportingAnswer } from "../crash-reporting";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

const WORD = /^[a-z0-9_]{1,64}$/;
const VARIANT = /^[A-Za-z0-9_]{0,64}$/;

/** One page signal as the core's, or null when any part is not a fixed word, flag or duration. */
export function telemetrySignal(native: Native, raw: unknown): AppTelemetrySignal | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const s = raw as Record<string, unknown>;
  const word = (k: string) => (typeof s[k] === "string" && WORD.test(s[k] as string) ? (s[k] as string) : null);
  const flag = (k: string) => (typeof s[k] === "boolean" ? (s[k] as boolean) : null);
  const ms = (k: string) => (typeof s[k] === "number" && Number.isInteger(s[k]) && (s[k] as number) >= 0 ? BigInt(s[k] as number) : null);
  const S = native.AppTelemetrySignal;
  switch (s.type) {
    case "feature": {
      const feature = word("feature");
      return feature ? new S.Feature({ feature }) : null;
    }
    case "step": {
      const step = word("step");
      const ok = flag("ok");
      return step && ok !== null ? new S.Step({ step, ok }) : null;
    }
    case "sign-in-failed": {
      const errorKind = word("errorKind");
      return errorKind ? new S.SignInFailed({ errorKind }) : null;
    }
    case "launched": {
      if (s.onboardingEligible === null || s.onboardingEligible === undefined) return new S.Launched({ onboardingEligible: undefined });
      const eligible = flag("onboardingEligible");
      return eligible === null ? null : new S.Launched({ onboardingEligible: eligible });
    }
    case "onboarding-page": {
      const [page, action, choice] = [word("page"), word("action"), word("choice")];
      return page && action && choice ? new S.OnboardingPage({ page, action, choice }) : null;
    }
    case "space-wizard": {
      const action = word("action");
      return action ? new S.SpaceWizard({ action }) : null;
    }
    case "space-create": {
      const [location, guestOs, kind, outcome, failedPhase] = [word("location"), word("guestOs"), word("kind"), word("outcome"), word("failedPhase")];
      const [stalled, gpu, elapsedMs] = [flag("stalled"), flag("gpu"), ms("elapsedMs")];
      const errorVariant = typeof s.errorVariant === "string" && VARIANT.test(s.errorVariant) ? s.errorVariant : null;
      // A retried create's final outcome; a page from before them sends neither.
      const retryAttempt = s.retryAttempt === undefined ? 0 : s.retryAttempt === 0 || s.retryAttempt === 1 ? s.retryAttempt : null;
      const errorSubclass = s.errorSubclass === undefined ? "none" : word("errorSubclass");
      if (!location || !guestOs || !kind || !outcome || !failedPhase || stalled === null || gpu === null || elapsedMs === null || errorVariant === null) return null;
      if (retryAttempt === null || !errorSubclass) return null;
      return new S.SpaceCreate({ location, guestOs, kind, outcome, failedPhase, stalled, elapsedMs, gpu, errorVariant, retryAttempt, errorSubclass });
    }
    case "space-create-started": {
      const [location, guestOs, kind, gpu] = [word("location"), word("guestOs"), word("kind"), flag("gpu")];
      return location && guestOs && kind && gpu !== null ? new S.SpaceCreateStarted({ location, guestOs, kind, gpu }) : null;
    }
    case "volume-setup": {
      const [surface, storage, mountMethod, outcome, addToFinder] = [word("surface"), word("storage"), word("mountMethod"), word("outcome"), flag("addToFinder")];
      return surface && storage && mountMethod && outcome && addToFinder !== null ? new S.VolumeSetup({ surface, storage, addToFinder, mountMethod, outcome }) : null;
    }
    case "share": {
      const [action, role, outcome] = [word("action"), word("role"), word("outcome")];
      // Why and where a share failed (CUA-1855); a page from before them sends neither.
      const errorKind = s.errorKind === undefined ? "none" : word("errorKind");
      const failedStage = s.failedStage === undefined ? "none" : word("failedStage");
      return action && role && outcome && errorKind && failedStage ? new S.Share({ action, role, outcome, errorKind, failedStage }) : null;
    }
    case "app-update": {
      const [action, channel, trigger] = [word("action"), word("channel"), word("trigger")];
      return action && channel && trigger ? new S.AppUpdate({ action, channel, trigger }) : null;
    }
    case "device-enroll": {
      const [method, outcome] = [word("method"), word("outcome")];
      return method && outcome ? new S.DeviceEnroll({ method, outcome }) : null;
    }
    case "experiment": {
      const [action, experiment] = [word("action"), word("experiment")];
      return action && experiment ? new S.Experiment({ action, experiment }) : null;
    }
    case "experiments-on": {
      const list = s.experiments;
      if (!Array.isArray(list) || list.length > 16) return null;
      if (!list.every((w) => typeof w === "string" && WORD.test(w))) return null;
      return new S.ExperimentsOn({ experiments: list as string[] });
    }
    default:
      return null;
  }
}

/**
 * The signals of a create, power or delete step (the core's
 * `telemetry.creates`) that this app's host records itself: Electron runs
 * every create in `AppModel.runCreate`, whose `sendCreate` records them.
 * The page's store derives the same signals from its own copy of the
 * creates, so recording both counted every create twice on Windows and
 * Linux (`cua_space_create`, `cua_space_create_started`,
 * `space_create_local`, `first_space_created`, `first_space_ready`). A
 * page that knows its host runs creates no longer sends them
 * (`HOST_RUNS_CREATES`); this keeps an older page from counting twice.
 */
export function hostRecorded(raw: unknown): boolean {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return false;
  const s = raw as Record<string, unknown>;
  switch (s.type) {
    case "space-create":
    case "space-create-started":
      return true;
    case "feature":
      return s.feature === "space_create_local" || s.feature === "space_create_cloud";
    case "step":
      return s.step === "first_space_created" || s.step === "first_space_ready";
    default:
      return false;
  }
}

/** The page's first run moved past Welcome (its usage notice was on
 * screen): any page left or shown other than Welcome being shown, or
 * "Set up later". */
export function leftWelcome(raw: unknown): boolean {
  if (!raw || typeof raw !== "object") return false;
  const s = raw as Record<string, unknown>;
  // Welcome being shown (the old `shown`, the funnel's `step_viewed`) is before the notice.
  if (s.type === "onboarding-page") return !(s.page === "welcome" && (s.action === "shown" || s.action === "step_viewed"));
  return s.type === "step" && s.step === "onboarding_skipped";
}

export function telemetryMethods({ model }: BridgeContext): Handlers {
  return {
    "telemetry.track": (args) => {
      if (!Array.isArray(args.signals)) throw Failure.badArgs("signals: array");
      const telemetry = model.telemetry;
      if (!telemetry || !telemetry.status().enabled) return null;
      const kept = args.signals.flatMap((raw) => {
        if (hostRecorded(raw)) return [];
        const signal = telemetrySignal(model.native, raw);
        return signal ? [{ raw, signal }] : [];
      });
      // Held until the notice was shown; leaving Welcome sends them in order.
      if (kept.length) telemetry.record(kept.map((k) => k.signal));
      if (kept.some((k) => leftWelcome(k.raw))) telemetry.acknowledgeNotice();
      return null;
    },
    // The one-time linking notice (CUA-1881): the SDK's words and whether
    // it is due. No telemetry (a test host) has none.
    // The page's own crash reports (apps/cua-spaces-web/src/crash-reporting): allowed only while usage data is on and its notice shown.
    "crashReporting.config": () => {
      const status = model.telemetry?.status();
      return crashReportingAnswer(status?.enabled === true && status.noticeShown !== false);
    },
    "telemetry.linkNotice": () => model.telemetry?.linkNotice() ?? null,
    "telemetry.acknowledgeLinkNotice": () => {
      model.telemetry?.acknowledgeLinkNotice();
      return null;
    },
  };
}

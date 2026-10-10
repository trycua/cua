// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Usage events from the page (`telemetry.track {signals}`): the core's
// fixed words, flags and durations, checked word by word, then recorded
// through the app's switch (the SwiftUI host's `telemetrySignal`). Off, or
// locked off, drops every signal here.
import type { Native } from "../native/load";
import type { AppTelemetrySignal } from "../native/generated/index";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

const WORD = /^[a-z0-9_]{1,64}$/;
/** An error enum case (`InsufficientDisk`), or empty when the shell has none. */
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
      if (!location || !guestOs || !kind || !outcome || !failedPhase || stalled === null || gpu === null || elapsedMs === null || errorVariant === null) return null;
      return new S.SpaceCreate({ location, guestOs, kind, outcome, failedPhase, stalled, elapsedMs, gpu, errorVariant });
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
      return action && role && outcome ? new S.Share({ action, role, outcome }) : null;
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

export function telemetryMethods({ model }: BridgeContext): Handlers {
  return {
    "telemetry.track": (args) => {
      if (!Array.isArray(args.signals)) throw Failure.badArgs("signals: array");
      const telemetry = model.telemetry;
      if (!telemetry || !telemetry.status().enabled) return null;
      const signals = args.signals.map((raw) => telemetrySignal(model.native, raw)).filter((s): s is AppTelemetrySignal => s !== null);
      if (signals.length) telemetry.record(signals);
      return null;
    },
  };
}

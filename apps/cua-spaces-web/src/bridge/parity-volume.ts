// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Cua Volume flows' part of the parity replay (`parity.ts`):
 * `drive-page`, `drive-onboarding` and `driver-card`. The core calls the
 * web makes for those screens go through `volume.ts` and `onboarding.ts`;
 * the states they reach are recorded, and `window.__cuaParity` can pin each
 * one on its screen (the Volume page, the first run, Settings' AI agents
 * card) for `e2e/volume.ts` to check.
 */

import type { CoreClient } from "./core";
import type { OnboardingFlowState, OnboardingView } from "./contracts/onboarding";
import type { AgentSetupOutcome, AgentSetupSummary, DriveFrame, DriveInput, DriveState, DriveView, DriverFrame } from "./contracts/volume";
import { onboardingCopy, writeSavedOnboarding } from "./onboarding";
import { driveFrame, driveInitial, driveReduce, driveStill, driveView, driverFrame, driverStill, setVolumePins, setupSummary } from "./volume";

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** A Cua Volume state a replay reached. */
export type VolumeCheckpoint =
  | { kind: "drive"; input: DriveInput; state: DriveState; view: DriveView }
  | { kind: "onboarding-drive"; state: OnboardingFlowState; view: OnboardingView }
  | { kind: "drive-frame"; tMs: number | "still"; frame: DriveFrame }
  | { kind: "driver-frame"; tMs: number | "still"; frame: DriverFrame }
  | { kind: "driver-copy"; agentsDriver: string; agentsDriverImage: string }
  | { kind: "driver-summary"; agent: string; name: string; outcomes: AgentSetupOutcome[]; summary: AgentSetupSummary };

/** The bridge's answer to each core method these screens call. */
export function volumeBridgeMethods(core: CoreClient, record: (c: VolumeCheckpoint) => void): Record<string, (a: Args) => unknown> {
  return {
    "drive.initial": () => driveInitial(core),
    "drive.reduce": (a) => driveReduce(core, a.state, a.action),
    "drive.view": (a) => {
      const view = driveView(core, a.input, a.state);
      record({ kind: "drive", input: a.input, state: a.state, view });
      return view;
    },
    // The first run: the web's `onboardingView` drops the native-only dots,
    // so the core's whole view is what the replay compares; the screen
    // check reads the Cua Volume card from it.
    "onboarding.view": (a) => {
      const view = core.call<OnboardingView>("onboarding.view", a);
      if (view.step === "drive") record({ kind: "onboarding-drive", state: a.state, view });
      return view;
    },
    "onboarding.drivePreviewFrame": (a) => {
      const frame = driveFrame(core, a.tMs)!;
      record({ kind: "drive-frame", tMs: a.tMs, frame });
      return frame;
    },
    "onboarding.drivePreviewStill": () => {
      const frame = driveStill(core)!;
      record({ kind: "drive-frame", tMs: "still", frame });
      return frame;
    },
    "onboarding.driverPreviewFrame": (a) => {
      const frame = driverFrame(core, a.tMs)!;
      record({ kind: "driver-frame", tMs: a.tMs, frame });
      return frame;
    },
    "onboarding.driverPreviewStill": () => {
      const frame = driverStill(core)!;
      record({ kind: "driver-frame", tMs: "still", frame });
      return frame;
    },
    "onboarding.copy": () => {
      const copy = onboardingCopy(core) as unknown as { agentsDriver: string; agentsDriverImage: string };
      record({ kind: "driver-copy", agentsDriver: copy.agentsDriver, agentsDriverImage: copy.agentsDriverImage });
      return copy;
    },
    "agents.setupSummary": (a) => {
      const summary = setupSummary(core, a.outcomes, a.agent, a.name);
      record({ kind: "driver-summary", agent: a.agent, name: a.name, outcomes: a.outcomes, summary });
      return summary;
    },
  };
}

/** What `window.__cuaParity` adds for these screens. */
export interface VolumeParityHandle {
  /** Pins the Volume page on a replay's input and state. */
  showVolume(input: DriveInput, state: DriveState): void;
  /** Puts the first run on a replay's state and holds the host's answers off. */
  showOnboarding(state: OnboardingFlowState): void;
  /** Pins both miniatures at `tMs` (or their stills) and the driver card's summaries. */
  showDriver(tMs: number | "still", summaries: AgentSetupSummary[] | null): void;
}

export function volumeParityHandle(): VolumeParityHandle {
  return {
    showVolume: (input, state) => setVolumePins({ volume: { input, state } }),
    showOnboarding: (state) => {
      setVolumePins({ onboarding: true });
      writeSavedOnboarding({ state, skipped: false });
    },
    showDriver: (tMs, summaries) => setVolumePins({ driver: { tMs, summaries } }),
  };
}

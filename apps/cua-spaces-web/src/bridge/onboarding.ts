// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run in the web UI: the app core's onboarding flow
 * (`onboarding.initial`, `.reduce`, `.view`, `.copy`), the same one the
 * SwiftUI and Tauri apps draw.
 *
 * One of the core's pages needs the native app, so a web host passes
 * through it with the answer a native user gets by not changing anything:
 * Menu bar (the notch), `presentation-picked` with the current setting.
 *
 * The pages left are Welcome (with the usage-data switch), Sign in, AI
 * agents (with the cua-driver card), Cua Volume (only with its experiment
 * on; `volume.ts` runs its commands), This machine (with the permissions
 * still to grant) and Done. The order, titles, buttons and answers are the
 * core's.
 *
 * Without the core, small TypeScript stand-ins keep the demo usable. They
 * are not the product's behaviour.
 */

import type { CoreClient } from "./core";
import type { OnboardingMode, PermissionHint, TelemetryView } from "./contracts/host";
import type {
  OnboardingAction,
  OnboardingCopy,
  OnboardingFlowState,
  OnboardingStep,
  OnboardingTelemetry,
  OnboardingView,
  PermissionRow,
  SandboxImage,
} from "./contracts/onboarding";
import type { SpaceOs } from "./contracts/spaces";

/** The core's pages a web host passes through (see above). */
export const NATIVE_ONLY_STEPS: readonly OnboardingStep[] = ["presentation"];

/** Done's summary facts about native-only pages; the web leaves them out. */
const NATIVE_ONLY_FACTS = new Set(["cua command", "Shows in"]);

/** What the pass-through answers need from the host. */
export interface OnboardingContext {
  /** The "Spaces tab in the notch" setting now (`menuBar`). */
  menuBar: boolean;
  /** Sees every step the core takes, with the state before it (usage events). */
  observe?: (before: OnboardingFlowState, action: OnboardingAction) => void;
}

export function onboardingInitial(core: CoreClient, identity: string | null, installerMode: OnboardingMode | null = null): OnboardingFlowState {
  return (
    core.tryCall<OnboardingFlowState>("onboarding.initial", { installerMode, identity }) ?? {
      step: "welcome",
      identity,
      installerMode,
      mode: null,
      menuBar: false,
      telemetry: null,
    }
  );
}

function reduceOnce(core: CoreClient, state: OnboardingFlowState, action: OnboardingAction, ctx?: OnboardingContext): OnboardingFlowState {
  ctx?.observe?.(state, action);
  return core.tryCall<OnboardingFlowState>("onboarding.reduce", { state, action }) ?? fallbackReduce(state, action);
}

/**
 * The core's reducer, then past any native-only page: forward with its
 * default answer, or on back when the action was Back.
 */
export function onboardingReduce(
  core: CoreClient,
  state: OnboardingFlowState,
  action: OnboardingAction,
  ctx: OnboardingContext,
): OnboardingFlowState {
  let s = reduceOnce(core, state, action, ctx);
  for (let guard = 0; guard < NATIVE_ONLY_STEPS.length + 1 && NATIVE_ONLY_STEPS.includes(s.step); guard++) {
    const before = s.step;
    if (action.type === "back") {
      s = reduceOnce(core, s, { type: "back" }, ctx);
    } else if (s.step === "agents") {
      s = reduceOnce(core, s, { type: "agents-done", configured: [] }, ctx);
    } else if (s.step === "presentation") {
      s = reduceOnce(core, s, { type: "presentation-picked", menuBar: ctx.menuBar }, ctx);
      s = reduceOnce(core, s, { type: "presentation-done" }, ctx);
    }
    if (s.step === before) break;
  }
  return s;
}

/** The page as drawn, without the native-only pages' dots and facts. */
export function onboardingView(core: CoreClient, state: OnboardingFlowState): OnboardingView {
  const v = core.tryCall<OnboardingView>("onboarding.view", { state }) ?? fallbackView(state);
  return {
    ...v,
    dots: v.dots.filter((d) => !NATIVE_ONLY_STEPS.includes(d.step)),
    summary: v.summary.filter((f) => !NATIVE_ONLY_FACTS.has(f.label)),
  };
}

/** The machine's telemetry setting as the flow reads it: locked when the
 * environment decides (the same rule as Settings, Privacy). */
export function onboardingTelemetry(t: TelemetryView): OnboardingTelemetry {
  const locked = ["do_not_track", "env", "legacy_env", "ci"].includes(t.sourceKind);
  return { enabled: t.enabled, lockedBy: locked ? t.source : null, noticeShown: t.noticeShown };
}

export function onboardingCopy(core: CoreClient): OnboardingCopy {
  return core.tryCall<OnboardingCopy>("onboarding.copy") ?? FALLBACK_COPY;
}

/** "Signed in as ada@example.com." */
export function signedInText(core: CoreClient, identity: string | null): string {
  return (
    core.tryCall<string>("onboarding.signedInText", { identity }) ?? (identity ? `Signed in as ${identity}.` : "Signed in.")
  );
}

/** "Confirm ABCD-EFGH in your browser." or "Finish in your browser." */
export function signInCodeText(core: CoreClient, userCode: string | null | undefined): string {
  return (
    core.tryCall<string>("onboarding.signInCodeText", { userCode: userCode ?? null }) ??
    (userCode ? `Confirm ${userCode} in your browser.` : "Finish in your browser.")
  );
}

/** The panes still to grant (`host.permissionRows`), from `host.status`'s hints. */
export function permissionRows(core: CoreClient, hints: PermissionHint[]): PermissionRow[] {
  const permissions = hints.map((h) => ({
    id: h.id,
    title: h.label,
    settingsUrl: h.settingsUrl ?? null,
    instructions: h.instructions ?? null,
    granted: Boolean(h.granted),
  }));
  return (
    core.tryCall<PermissionRow[]>("host.permissionRows", { permissions }) ??
    permissions
      .filter((p) => !p.granted)
      .map((p) => ({ id: p.id, title: p.title, help: p.instructions ?? "", settingsUrl: p.settingsUrl || null }))
  );
}

/**
 * One image per OS for the first Space: the core's picker catalog, first
 * published full image of each (the New Space picker's first rows).
 */
export function firstSpaceImages(core: CoreClient): SandboxImage[] {
  const all = core.tryCall<SandboxImage[]>("wizard.pickerImages") ?? FALLBACK_IMAGES;
  const order: SpaceOs[] = ["macos", "linux", "windows"];
  return order.flatMap((os) => {
    const published = all.filter((i) => i.published && i.os === os && i.group === "canonical");
    const pick = published.find((i) => i.tier !== "slim") ?? published[0];
    return pick ? [pick] : [];
  });
}

/* ---- Stand-ins without the core ------------------------------------------- */

const ORDER: OnboardingStep[] = ["welcome", "signin", "agents", "presentation", "mode", "done"];
const LABELS: Record<OnboardingStep, string> = {
  welcome: "Welcome",
  signin: "Sign in",
  agents: "AI agents",
  presentation: "Menu bar",
  drive: "Cua Volume",
  mode: "This machine",
  done: "Done",
};

function fallbackReduce(s: OnboardingFlowState, a: OnboardingAction): OnboardingFlowState {
  const at = (step: OnboardingStep) => s.step === step;
  switch (a.type) {
    case "start":
      return at("welcome") ? { ...s, step: "signin", runCounted: true } : s;
    case "signed-in":
      return { ...s, identity: a.identity };
    case "signin-done":
      return at("signin") ? { ...s, step: "agents" } : s;
    case "agents-done":
      return at("agents") ? { ...s, agents: a.configured, step: "presentation" } : s;
    case "presentation-picked":
      return at("presentation") ? { ...s, menuBar: a.menuBar } : s;
    case "presentation-done":
      return at("presentation") ? { ...s, step: "mode" } : s;
    case "mode-chosen":
      return at("mode") ? { ...s, mode: a.mode, step: "done" } : s;
    case "telemetry-loaded":
      return { ...s, telemetry: a.telemetry };
    case "usage-data-toggled":
      return at("welcome") && !s.telemetry?.lockedBy ? { ...s, telemetry: { ...s.telemetry, enabled: a.on } } : s;
    case "back": {
      const i = ORDER.indexOf(s.step);
      return i > 0 ? { ...s, step: ORDER[i - 1]! } : s;
    }
    default:
      // The Cua Volume page needs the core (its experiment never loads without it).
      return s;
  }
}

function fallbackView(s: OnboardingFlowState): OnboardingView {
  const pages: Partial<Record<OnboardingStep, [string, string, string]>> = {
    welcome: ["Welcome to Cua Spaces", "Computers for you and your agents.", "Get started"],
    signin: ["Sign in", "Connect your machines and your team.", "Continue"],
    mode: ["How will you use this machine?", "You can change this later.", "Continue"],
    done: ["You're all set", "Cua Spaces is in your menu bar.", "Start using Cua Spaces"],
  };
  const [title, lede, primaryLabel] = pages[s.step] ?? ["", "", "Continue"];
  const pre = s.installerMode ?? "client";
  const locked = s.telemetry?.lockedBy || null;
  return {
    step: s.step,
    dots: ORDER.map((step) => ({ step, label: LABELS[step], current: step === s.step })),
    title,
    lede,
    primaryLabel,
    canSkip: s.step === "agents" || (s.step === "signin" && !s.identity),
    canBack: s.step !== "welcome",
    showMark: s.step === "welcome",
    preselectedMode: pre,
    summary:
      s.step === "done"
        ? [
            { label: "Account", value: s.identity ?? "not signed in" },
            { label: "AI agents", value: Array.isArray(s.agents) && s.agents.length ? s.agents.join(", ") : "none" },
            { label: "This machine", value: s.mode === "host" ? "Set up for unattended access" : "Access other machines" },
          ]
        : [],
    choices:
      s.step === "mode"
        ? [
            { mode: "client", label: "Access other machines", preselected: pre === "client" },
            { mode: "host", label: "Set up this machine for unattended access", preselected: pre === "host" },
          ]
        : [],
    notice: s.step === "welcome" ? "Cua collects anonymous usage data. Change it here or in Settings, Privacy." : null,
    noticeLinkLabel: s.step === "welcome" ? "What is collected" : null,
    noticeLinkUrl: s.step === "welcome" ? "https://cua.ai/docs/cua-sdk/concepts/telemetry" : null,
    usage:
      s.step === "welcome"
        ? {
            label: "Share anonymous usage data",
            on: s.telemetry?.enabled ?? true,
            enabled: !locked,
            help: locked ? `Set by ${locked}` : null,
          }
        : null,
  };
}

const FALLBACK_COPY: OnboardingCopy = {
  back: "Back",
  skip: "Skip",
  continueLabel: "Continue",
  tryAgain: "Try again",
  signIn: "Sign in",
  signInWaiting: "Waiting for the browser…",
  permissionsTitle: "Grant in System Settings",
  openSettings: "Open Settings",
  teams: "Teams · coming soon",
  teamsLink: "Join the waitlist",
  teamsUrl: "https://cua.ai/teams",
};

const FALLBACK_IMAGES: SandboxImage[] = [
  { ref: "ghcr.io/trycua/macos:26", group: "canonical", os: "macos", name: "macOS Tahoe 26", variant: "vm", summary: "", published: true },
  { ref: "ghcr.io/trycua/linux:24.04", group: "canonical", os: "linux", name: "Ubuntu 24.04", variant: "container", summary: "", published: true },
  { ref: "ghcr.io/trycua/windows:2022", group: "canonical", os: "windows", name: "Windows Server 2022", variant: "vm", summary: "", published: true },
];

/* ---- Saved progress (resume) ------------------------------------------------ */

/** Where the flow stands between launches: the core's state, and whether
 * the user skipped the rest for now (the shell stops opening it). */
export interface SavedOnboarding {
  state: OnboardingFlowState | null;
  skipped: boolean;
}

const STORAGE_KEY = "cua-spaces:onboarding";
const EMPTY: SavedOnboarding = { state: null, skipped: false };
let saved: SavedOnboarding | undefined;
const listeners = new Set<() => void>();

function storage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

/** The saved progress (shared by every hook on the page). */
export function readSavedOnboarding(): SavedOnboarding {
  if (saved) return saved;
  try {
    const raw = storage()?.getItem(STORAGE_KEY);
    const parsed = raw ? (JSON.parse(raw) as Partial<SavedOnboarding>) : null;
    saved = parsed && typeof parsed === "object" ? { state: parsed.state ?? null, skipped: Boolean(parsed.skipped) } : EMPTY;
  } catch {
    saved = EMPTY;
  }
  return saved;
}

export function writeSavedOnboarding(next: SavedOnboarding | null): void {
  saved = next ?? EMPTY;
  try {
    if (next) storage()?.setItem(STORAGE_KEY, JSON.stringify(next));
    else storage()?.removeItem(STORAGE_KEY);
  } catch {
    // Private mode or a full disk: the flow still works, it just won't resume.
  }
  for (const l of listeners) l();
}

export function subscribeSavedOnboarding(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Tests: forget the cached copy. */
export function resetSavedOnboardingCache(): void {
  saved = undefined;
}

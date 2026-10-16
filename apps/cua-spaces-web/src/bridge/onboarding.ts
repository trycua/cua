// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run in the web UI: the app core's onboarding flow
 * (`onboarding.initial`, `.reduce`, `.view`, `.copy`), the same one the
 * SwiftUI and Tauri apps draw.
 *
 * The pages are Welcome (with the usage-data switch), Sign in, AI agents
 * (with the cua-driver card), Menu bar (where the app shows: the notch and
 * the menu bar, or the menu bar only), Cua Volume (only with its experiment
 * on; `volume.ts` runs its commands), This machine (with the permissions
 * still to grant) and Done. The order, titles, buttons and answers are the
 * core's.
 *
 * Menu bar is a Mac's: the notch is the macOS app's (the SwiftUI app, and
 * the Electron app's notch helper). Off a Mac there is no notch, so the
 * flow passes through that page with the answer a user gets by not
 * changing anything, `presentation-picked` with the current setting.
 *
 * Without the core, small TypeScript stand-ins keep the demo usable. They
 * are not the product's behaviour.
 */

import type { CoreClient } from "./core";
import { hostOs, type HostOs } from "./host-os";
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

/** Whether the host has the notch, so the Menu bar page has a choice to
 * offer: the macOS app's (the same rule as Settings' "Spaces tab in the
 * notch" row). */
export function hasNotch(os: HostOs = hostOs()): boolean {
  return os === "macos";
}

/** The core's pages this host passes through (see above). */
export function passedSteps(os: HostOs = hostOs()): readonly OnboardingStep[] {
  return hasNotch(os) ? [] : ["presentation"];
}

/** Done's summary facts about pages the web passes through: the `cua`
 * install is the SwiftUI app's own page, Shows in follows Menu bar. */
function passedFacts(os: HostOs): Set<string> {
  return new Set(hasNotch(os) ? ["cua command"] : ["cua command", "Shows in"]);
}

/** What the pass-through answers need from the host. */
export interface OnboardingContext {
  /** The "Spaces tab in the notch" setting now (`menuBar`). */
  menuBar: boolean;
  /** The system the app runs on (default: this host's). */
  os?: HostOs;
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
 * The core's reducer, then past any page this host passes through: forward
 * with its default answer, or on back when the action was Back. Landing on
 * Menu bar, the choice starts from the setting now (it may have changed in
 * Settings since the flow started), as the SwiftUI page does on appear.
 */
export function onboardingReduce(
  core: CoreClient,
  state: OnboardingFlowState,
  action: OnboardingAction,
  ctx: OnboardingContext,
): OnboardingFlowState {
  const passed = passedSteps(ctx.os);
  let s = reduceOnce(core, state, action, ctx);
  for (let guard = 0; guard < passed.length + 1 && passed.includes(s.step); guard++) {
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
  if (s.step === "presentation" && state.step !== "presentation" && s.menuBar !== ctx.menuBar) {
    s = reduceOnce(core, s, { type: "presentation-picked", menuBar: ctx.menuBar }, ctx);
  }
  return s;
}

/** The page as drawn, without the dots and facts of the pages this host
 * passes through, in the words of the system the app runs on (the tray off
 * a Mac). */
export function onboardingView(core: CoreClient, state: OnboardingFlowState, os: HostOs = hostOs()): OnboardingView {
  const v = core.tryCall<OnboardingView>("onboarding.view", { state, hostOs: os }) ?? fallbackView(state, os);
  const passed = passedSteps(os);
  const facts = passedFacts(os);
  return {
    ...v,
    dots: v.dots.filter((d) => !passed.includes(d.step)),
    summary: v.summary.filter((f) => !facts.has(f.label)),
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
 * published full image of each (the New Space picker's first rows). No
 * macOS image off macOS.
 */
export function firstSpaceImages(core: CoreClient, os: HostOs = hostOs()): SandboxImage[] {
  const all = core.tryCall<SandboxImage[]>("wizard.pickerImages") ?? FALLBACK_IMAGES;
  // macOS images run on Lume (Apple silicon): not offered on Linux or Windows.
  const order: SpaceOs[] = os === "macos" ? ["macos", "linux", "windows"] : ["linux", "windows"];
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

function fallbackView(s: OnboardingFlowState, os: HostOs): OnboardingView {
  const pages: Partial<Record<OnboardingStep, [string, string, string]>> = {
    welcome: ["Welcome to Cua Spaces", "Computers for you and your agents.", "Get started"],
    signin: ["Sign in", "Sign in for account-based access to your machines and Spaces across devices; skip to continue without it.", "Continue"],
    presentation: ["Where should Cua Spaces show up?", "You can change this in Settings.", "Continue"],
    mode: ["How will you use this machine?", "You can change this later.", "Continue"],
    done: ["You're all set", os === "macos" ? "Cua Spaces is in your menu bar." : "Cua Spaces is in your system tray.", "Start using Cua Spaces"],
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
            { label: "Shows in", value: s.menuBar ? "Menu bar only" : "Notch and menu bar" },
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
    presentations:
      s.step === "presentation"
        ? [
            { menuBar: false, title: "Notch and menu bar", id: "onboarding-notch", imageLabel: "The Spaces tiles open from the notch, and the menu bar menu", selected: !s.menuBar },
            { menuBar: true, title: "Menu bar only", id: "onboarding-menu-bar", imageLabel: "The menu bar menu alone", selected: s.menuBar },
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
  teams: "Teams · manage them at app.cua.ai",
  teamsLink: "Open the account app",
  teamsUrl: "https://app.cua.ai/welcome",
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

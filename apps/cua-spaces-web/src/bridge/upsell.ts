// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The in-app Pro and Teams prompts. The words, the plan each one fits and
 * the rules (one prompt every 3 days, not during a live session or an agent
 * run, a closed prompt stays closed for 30 days) are the app core's
 * (`upsell.prompt`, `upsell.allows`, `cua-spaces-app-core/src/upsell.rs`);
 * this keeps what the page remembers and draws nothing.
 *
 * Usage events: `cua_spaces_feature_used` `upsell_<surface>_<action>`
 * (`telemetry.upsell`): `meter`, `share` and `machine` shown, click or
 * dismiss; `plan_card` and `account_menu` click. A click is also
 * `cua_upgrade_link_opened` and the meter past a limit
 * `cua_plan_limit_shown` (CUA-1881); a refused create or connect and a team
 * step over the plan record those through `usePlanLimitTelemetry`. Every
 * upgrade link opens with `?from=<surface>` (the core's `upgrade.link`), so
 * the account app's purchase funnel knows where it started.
 */

import { useCallback, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import { useFlag } from "./flags";
import { useAgents, useSettings } from "./hooks";
import type { RelayUsage } from "./ops/relay-usage";
import { useRelayUsage } from "./relay-usage";

export type UpsellSurface = "meter" | "share" | "machine";
/** What stopped the person, for a limit card (`upsell.card`). */
export type UpsellCardReason = "hours" | "sessions" | "invite" | "seats" | "machines";
export type UpsellAction = "shown" | "click" | "dismiss";

/** `upsell::UpsellPrompt`. */
export interface UpsellPrompt {
  surface: UpsellSurface;
  text: string;
  cta: { label: string; url: string };
  /** "Not now". */
  dismiss: string;
}

/** `upsell::UpsellState`. */
interface UpsellState {
  nowMs: number;
  lastPromptMs: number | null;
  dismissedMs: number | null;
  quiet: boolean;
}

const LAST_KEY = "cua.upsell.lastPromptAt";
const dismissedKey = (s: UpsellSurface) => `cua.upsell.dismissed.${s}`;

const listeners = new Set<() => void>();

/** How long a shown prompt stays through remounts (a page redrawing, a
 * sheet reopened) before the rules apply to it again. */
const HOLD_MS = 10 * 60_000;
/** Prompts on screen now, by surface: until when. */
const holds = new Map<UpsellSurface, number>();
const isHeld = (surface: UpsellSurface) => (holds.get(surface) ?? 0) > Date.now();

function readMs(key: string): number | null {
  try {
    const v = Number(globalThis.localStorage?.getItem(key));
    return Number.isFinite(v) && v > 0 ? v : null;
  } catch {
    return null;
  }
}

function writeMs(key: string, ms: number): void {
  try {
    globalThis.localStorage?.setItem(key, String(ms));
  } catch {
    // Not kept: the rules read it as never shown.
  }
  for (const l of [...listeners]) l();
}

/** Forget what was shown and closed (tests). */
export function resetUpsellMemory(): void {
  try {
    holds.clear();
    for (const k of [LAST_KEY, dismissedKey("meter"), dismissedKey("share"), dismissedKey("machine")]) globalThis.localStorage?.removeItem(k);
  } catch {
    // Nothing kept.
  }
  for (const l of [...listeners]) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

/** A live session (a relay connection) or an agent run is going. */
export function useQuiet(): boolean {
  const usage = useRelayUsage();
  const agents = useAgents();
  return (usage?.liveSessions ?? 0) > 0 || (agents.data?.runs ?? []).some((r) => r.status === "running");
}

/** For usage data: the cap of a limit (`sessions`, `hours`, ...) and the plan. */
export interface PlanFacts {
  kind?: string | null;
  plan?: string | null;
}

type TrackSurface = UpsellSurface | "plan_card" | "account_menu" | "onboarding" | `card_${UpsellCardReason}`;

/** Counts the event; false when it could not be (no store yet). */
function useTrack(): (surface: TrackSurface, action: UpsellAction, facts?: PlanFacts) => boolean {
  const ctx = useContext(BridgeContext);
  return useCallback(
    (surface, action, facts) => {
      const store = ctx?.store;
      if (!store) return false;
      const args: Record<string, string> = { surface, action };
      if (facts?.kind) args.kind = facts.kind;
      if (facts?.plan) args.plan = facts.plan;
      const signals = ctx.core.tryCall<unknown[]>("telemetry.upsell", args);
      if (signals?.length) void store.telemetry.track(signals);
      return true;
    },
    [ctx],
  );
}

/** One click on an always-visible plan link (the plan card, the account menu). */
export function useTrackPlanClick(): (surface: "plan_card" | "account_menu") => void {
  const track = useTrack();
  const plan = useRelayUsage()?.plan ?? null;
  return useCallback((surface) => track(surface, "click", { plan }), [track, plan]);
}

/** Where a plan limit shows, or an upgrade link opens, outside the prompts above. */
export type PlanLimitSurface = "app_create" | "app_connect" | "app_team" | "app_meter" | "app_share" | "app_machine" | "app_plan_card" | "app_account_menu" | "app_onboarding";

/** `url` with `?from=<surface>`, the rest kept (the core's `upgrade.link`); `url` itself without the core. */
export function upgradeLink(core: CoreClient | null | undefined, url: string, surface: PlanLimitSurface): string {
  try {
    return core?.tryCall<string>("upgrade.link", { url, from: surface }) ?? url;
  } catch {
    return url;
  }
}

export interface PlanLimitTelemetry {
  /** A plan limit is on screen (`cua_plan_limit_shown`). */
  shown(surface: PlanLimitSurface, facts: PlanFacts): void;
  /** Its upgrade link was opened (`cua_upgrade_link_opened`). */
  opened(surface: PlanLimitSurface, facts: PlanFacts): void;
  /** The upgrade link to open, saying where (`?from=<surface>`). */
  link(url: string, surface: PlanLimitSurface): string;
}

/** Usage events and links for plan limits (the core's `telemetry.planLimit` and `upgrade.link`). */
export function usePlanLimitTelemetry(): PlanLimitTelemetry {
  const ctx = useContext(BridgeContext);
  return useMemo(() => {
    const send = (action: "shown" | "opened", surface: PlanLimitSurface, facts: PlanFacts) => {
      const store = ctx?.store;
      if (!store) return;
      try {
        const signals = ctx.core.tryCall<unknown[]>("telemetry.planLimit", { action, surface, kind: facts.kind ?? null, plan: facts.plan ?? null });
        if (signals?.length) void store.telemetry.track(signals);
      } catch {
        // An older core: no event.
      }
    };
    return {
      shown: (surface, facts) => send("shown", surface, facts),
      opened: (surface, facts) => send("opened", surface, facts),
      link: (url, surface) => upgradeLink(ctx?.core, url, surface),
    };
  }, [ctx]);
}

export interface UpsellGate {
  /** Draw the prompt now. */
  show: boolean;
  click(): void;
  dismiss(): void;
}

/**
 * Whether the `surface` prompt may show, and its events. `candidate`: the
 * moment is here (a notice exists, a share just worked). `gated`: it is a
 * prompt, so the rules apply; the `limit` notice is the status and is not.
 * Once shown it stays until closed or until a live session or an agent run
 * starts. Without the core nothing shows.
 */
export function useUpsellGate(surface: UpsellSurface, candidate: boolean, gated = true, facts: PlanFacts = {}): UpsellGate {
  const kind = facts.kind ?? null;
  const plan = facts.plan ?? null;
  const ctx = useContext(BridgeContext);
  const core = ctx?.core;
  const quiet = useQuiet();
  const track = useTrack();
  const memory = useSyncExternalStore(subscribe, () => `${readMs(LAST_KEY)}:${readMs(dismissedKey(surface))}`, () => "");
  const [held, setHeld] = useState(() => isHeld(surface));
  const [closed, setClosed] = useState(false);
  const tracked = useRef(false);

  const allowed =
    !gated ||
    (core?.tryCall<boolean>("upsell.allows", {
      state: { nowMs: Date.now(), lastPromptMs: readMs(LAST_KEY), dismissedMs: readMs(dismissedKey(surface)), quiet } satisfies UpsellState,
    }) ?? false);

  useEffect(() => {
    if (held || closed || !candidate || !allowed) return;
    holds.set(surface, Date.now() + HOLD_MS);
    setHeld(true);
    if (gated) writeMs(LAST_KEY, Date.now());
    if (!tracked.current) {
      tracked.current = true;
      track(surface, "shown", { kind, plan });
    }
    // `memory` re-reads the rules when another prompt was shown.
  }, [held, closed, candidate, allowed, gated, surface, track, memory, kind, plan]);

  const show = held && !closed && candidate && !(gated && quiet);
  return {
    show,
    click: () => track(surface, "click", { kind, plan }),
    dismiss: () => {
      track(surface, "dismiss", { kind, plan });
      holds.delete(surface);
      // The meter holds its own dismissal per month and level (`dismissKey`).
      if (surface !== "meter") writeMs(dismissedKey(surface), Date.now());
      setClosed(true);
    },
  };
}

/** The `share` or `machine` prompt's words, when its moment fits the plan. */
export function useUpsellPrompt(surface: "share" | "machine", usage: RelayUsage | null): UpsellPrompt | null {
  const core = useContext(BridgeContext)?.core;
  const accountApp = useSettings().data?.accountUrl ?? null;
  if (!core || !usage) return null;
  return (
    core.tryCall<UpsellPrompt | null>("upsell.prompt", {
      surface,
      usage,
      ...(accountApp ? { accountApp } : {}),
      state: { nowMs: Date.now(), lastPromptMs: null, dismissedMs: null, quiet: false },
    }) ?? null
  );
}

/** `upsell::UpsellCard`. */
export interface UpsellCard {
  reason: UpsellCardReason;
  title: string;
  body: string;
  price: string;
  cta: { label: string; url: string };
  /** "Not now". */
  dismiss: string;
}

function cardFor(core: CoreClient, reason: UpsellCardReason, usage: Partial<RelayUsage> | null, accountApp: string | null): UpsellCard | null {
  return (
    core.tryCall<UpsellCard | null>("upsell.card", {
      reason,
      usage: { plan: "free", ...usage },
      ...(accountApp ? { accountApp } : {}),
    }) ?? null
  );
}

/** Whether the flag is on and the core has a card for `reason` on this plan (draws and counts nothing). */
export function useUpsellCardAvailable(reason: UpsellCardReason | null, usage: Partial<RelayUsage> | null): boolean {
  const core = useContext(BridgeContext)?.core;
  const on = useFlag("upsell_cards");
  const accountApp = useSettings().data?.accountUrl ?? null;
  return Boolean(on && core && reason && cardFor(core, reason, usage, accountApp));
}

export interface UpsellCardState {
  /** The card to draw now; null when the flag is off, there is nothing to
   * offer, the core is missing or it was closed. */
  card: UpsellCard | null;
  click(): void;
  dismiss(): void;
}

/**
 * The limit card for `reason`: what happened, what the plan gives, the price,
 * Upgrade and Not now. It is for the moment a limit stops the person, so it
 * is behind the `upsell_cards` flag (off unless turned on) and does not wait
 * for the 3-day gap. `reason` null: no limit here. `usage` carries the plan
 * (none known: Free). Events: `upsell_card_<reason>_shown | click | dismiss`,
 * `shown` once per card drawn.
 */
export function useUpsellCard(reason: UpsellCardReason | null, usage: Partial<RelayUsage> | null): UpsellCardState {
  const ctx = useContext(BridgeContext);
  const core = ctx?.core;
  const on = useFlag("upsell_cards");
  const accountApp = useSettings().data?.accountUrl ?? null;
  const track = useTrack();
  const [closed, setClosed] = useState<UpsellCardReason | null>(null);
  const shown = useRef<UpsellCardReason | null>(null);
  const card = on && core && reason && closed !== reason ? cardFor(core, reason, usage, accountApp) : null;
  const shownReason = card?.reason ?? null;
  useEffect(() => {
    if (!shownReason || shown.current === shownReason) return;
    if (track(`card_${shownReason}`, "shown")) shown.current = shownReason;
  }, [shownReason, track]);
  return {
    card,
    click: () => reason && track(`card_${reason}`, "click"),
    dismiss: () => {
      if (!reason) return;
      track(`card_${reason}`, "dismiss");
      setClosed(reason);
    },
  };
}

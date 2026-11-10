// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The agent the ask box starts a task with: a coding app, one of the
 * accounts (sign-ins) the person has for it and, when the account offers a
 * choice, a model. What each app can run on is the core's
 * (`connectors.agents`); this finds the last pick again and names it.
 *
 * What is kept: the account is the app's own choice, saved by the host
 * (`agentKeys.use`) so it outlives the page and is the one a run uses; the
 * app and the model are kept on this computer. A remembered account that
 * is no longer saved is never used: the app's own choice, else its first
 * account, stands in. With nothing remembered, the first connected app.
 */

import { useSyncExternalStore } from "react";

import type { AgentAccount, AgentGroup, AgentsView, ModelChoice } from "@/bridge";

import { chipWords, effortOf } from "./model-effort";

/** What was picked: an app and, when the person chose, its account and model. */
export interface PickedAgent {
  harness: string;
  model?: string;
  /** The reasoning level the person chose for the model (kept only while the model offers it). */
  effort?: string;
  env?: string;
  slot?: string;
}

/** A pick with the rows it names. */
export interface AgentSelection {
  group: AgentGroup;
  account: AgentAccount;
  model: ModelChoice;
  /** The harness id a run uses. */
  harness: string;
  /** The reasoning level in use, null for a model without levels. */
  effort: string | null;
}

const sameAccount = (a: AgentAccount, p: { env?: string; slot?: string | null }) => a.env === p.env && (a.slot ?? null) === (p.slot ?? null);

/** The account an app runs with: the remembered one while it is still saved, else the app's own choice, else its first. */
function accountOf(group: AgentGroup, p: PickedAgent | null): AgentAccount | null {
  const remembered = p?.harness === group.harness && p.env ? group.accounts.find((a) => sameAccount(a, p)) : undefined;
  return remembered ?? group.accounts.find((a) => a.selected) ?? group.accounts[0] ?? null;
}

function select(group: AgentGroup | undefined, p: PickedAgent | null, wanted?: string | null): AgentSelection | null {
  if (!group || group.state !== "connected") return null;
  const account = accountOf(group, p);
  if (!account) return null;
  const id = [wanted, p?.harness === group.harness ? p.model : undefined].find((m) => m && account.models.some((x) => x.id === m)) ?? account.defaultModel;
  const model = account.models.find((m) => m.id === id) ?? account.models[0];
  return model
    ? { group, account, model, harness: group.harness, effort: effortOf(group.harness, model.id, p?.harness === group.harness ? p.effort : undefined) }
    : null;
}

/**
 * The agent to start with: the person's last pick while it is still offered, else the first connected app
 * (the menu's order) with its own account and quick model, else nothing.
 */
export function resolveAgent(view: AgentsView | null | undefined, picked: PickedAgent | null): AgentSelection | null {
  if (!view) return null;
  const last = picked
    ? select(
        view.groups.find((g) => g.harness === picked.harness),
        picked,
      )
    : null;
  if (last) return last;
  const d = view.default;
  return d
    ? select(
        view.groups.find((g) => g.harness === d.harness),
        { harness: d.harness, env: d.env, ...(d.slot ? { slot: d.slot } : {}) },
        d.model,
      )
    : null;
}

/** What picking `account` of `group` starts with: that account and its quick model (the last model while it is offered). */
export function pickAccount(group: AgentGroup, account: AgentAccount, picked: PickedAgent | null): PickedAgent {
  const keep = picked?.harness === group.harness && picked.model && account.models.some((m) => m.id === picked.model) ? picked.model : account.defaultModel;
  return { harness: group.harness, env: account.env, ...(account.slot ? { slot: account.slot } : {}), ...(keep ? { model: keep } : {}) };
}

/** The pick as a line of words: the app ("Claude Code"). The account is chosen inside the menu. */
export const agentLine = (s: AgentSelection) => s.group.label;

/** The pick as the chip says it: "Sonnet 5.5" and a quiet "High", or "OpenCode" and a quiet model. The logo already names the app. */
export const chipLine = (s: AgentSelection, say: (key: string) => string) => chipWords(s.group.label, s.model.label, s.effort, say);

/** The pick to remember for a selection. */
export const pickOf = (s: AgentSelection): PickedAgent => ({
  harness: s.harness,
  model: s.model.id,
  env: s.account.env,
  ...(s.account.slot ? { slot: s.account.slot } : {}),
  ...(s.effort ? { effort: s.effort } : {}),
});

/* ---- the last pick, per computer ---- */

const KEY = "cua.askFirst.agent";
let cache: PickedAgent | null | undefined;
const listeners = new Set<() => void>();

function load(): PickedAgent | null {
  if (cache !== undefined) return cache;
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    const v = raw ? (JSON.parse(raw) as Partial<PickedAgent>) : null;
    cache =
      v && typeof v.harness === "string"
        ? {
            harness: v.harness,
            ...(typeof v.model === "string" ? { model: v.model } : {}),
            ...(typeof v.env === "string" ? { env: v.env } : {}),
            ...(typeof v.slot === "string" ? { slot: v.slot } : {}),
          }
        : null;
  } catch {
    cache = null;
  }
  return cache;
}

export function pickedAgent(): PickedAgent | null {
  return load();
}

export function rememberAgent(p: PickedAgent): void {
  cache = p;
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(p));
  } catch {
    // Kept for this session only.
  }
  for (const l of [...listeners]) l();
}

/** For tests: forget the cached pick. */
export function resetPickedAgent(): void {
  cache = undefined;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/** The last picked agent (re-renders when it changes). */
export function usePickedAgent(): PickedAgent | null {
  return useSyncExternalStore(subscribe, load, () => null);
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The model the ask box starts a task with, and which coding app runs it.
 * The person picks a model (and, optionally, a repo); the app that runs it
 * follows: a Claude sign-in is Claude Code's and a ChatGPT sign-in is Codex's,
 * a key goes to the app that reads it best. The last pick is kept on this
 * device, so most of the time nothing is touched and the person just types.
 */

import { useSyncExternalStore } from "react";

import type { ModelChoice, ModelGroup, ModelsView } from "@/bridge";

/** What was picked: a provider and one of its models. */
export interface PickedModel {
  provider: string;
  model: string;
}

/** The app that runs a provider's models when no sign-in names one (an API key). */
const PROVIDER_APP: Record<string, string> = { anthropic: "claude-code", openai: "openai-codex", google: "gemini-cli" };
/** The general app that runs every other provider's models. */
export const GENERAL_APP = "opencode";

/** The coding app (harness id) that runs a group's models. */
export function harnessFor(group: Pick<ModelGroup, "provider" | "harness">): string {
  return group.harness ?? PROVIDER_APP[group.provider] ?? GENERAL_APP;
}

/** A picked model with the group and row it names. */
export interface ModelSelection {
  group: ModelGroup;
  model: ModelChoice;
  harness: string;
}

/**
 * The model to start with: the person's last pick while it is still offered
 * (connected), else the host's default (the first fast model), else nothing.
 */
export function resolveModel(view: ModelsView | null | undefined, picked: PickedModel | null): ModelSelection | null {
  if (!view) return null;
  const find = (p: PickedModel | null | undefined) => {
    if (!p) return null;
    const group = view.groups.find((g) => g.provider === p.provider && g.state === "connected");
    const model = group?.models.find((m) => m.id === p.model);
    return group && model ? { group, model, harness: harnessFor(group) } : null;
  };
  return find(picked) ?? find(view.default);
}

/** A model's name with its provider, as one plain line ("Claude Haiku 4.5, Anthropic"). */
export const modelLine = (s: ModelSelection) => `${s.model.label}, ${s.group.label}`;

/* ---- the last pick, per device ---- */

const KEY = "cua.askFirst.model";
let cache: PickedModel | null | undefined;
const listeners = new Set<() => void>();

function load(): PickedModel | null {
  if (cache !== undefined) return cache;
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    const v = raw ? (JSON.parse(raw) as Partial<PickedModel>) : null;
    cache = v && typeof v.provider === "string" && typeof v.model === "string" ? { provider: v.provider, model: v.model } : null;
  } catch {
    cache = null;
  }
  return cache;
}

export function pickedModel(): PickedModel | null {
  return load();
}

export function rememberModel(p: PickedModel): void {
  cache = p;
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(p));
  } catch {
    // Kept for this session only.
  }
  for (const l of [...listeners]) l();
}

/** For tests: forget the cached pick. */
export function resetPickedModel(): void {
  cache = undefined;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/** The last picked model (re-renders when it changes). */
export function usePickedModel(): PickedModel | null {
  return useSyncExternalStore(subscribe, load, () => null);
}

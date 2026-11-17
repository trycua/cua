// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The account's default Space (CUA-3413): where new tasks from Home run
 * unless you pick another, and the order to try when it's offline. The
 * relay keeps it per account (`GET`/`PUT /v1/spaces/settings`), so the
 * apps, the CLI, the phone and agents agree; the relay also resolves it
 * (`GET /v1/spaces/default?here=<this machine>`), and every client uses
 * that answer.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `spaces.settings` | the host method `spaces.settings` | the host method `spaces.settings` | unsupported |
 * | `spaces.setSettings` | the host method `spaces.setSettings` | the host method `spaces.setSettings` | unsupported |
 * | `spaces.resolveDefault` | the host method `spaces.resolveDefault` | the host method `spaces.resolveDefault` | unsupported |
 *
 * Both native hosts call the SDK's `Devices.spaces_settings`,
 * `set_spaces_settings` and `resolve_default_space` from this enrolled
 * device (the resolution fills `here` with this computer's own machine id).
 * Null: signed out, or not enrolled. Nothing here ever creates a Space: a
 * new computer is made only when the person picks "New Space on ...".
 */

import type { OpCoverage } from "../coverage";

/** The stored setting (`cua_sdk::SpacesSettings`, camelCase). */
export interface SpacesSettings {
  /** A Space id (`relay:<machine>` or `relay:<machine>/<name>`), or null when unset. */
  defaultSpace: string | null;
  /** The order to try when the default is offline. Empty: every online computer, oldest first. */
  fallback: string[];
  /** False: only the default, then this computer. */
  fallbackOn: boolean;
  /** Unix seconds, set by the relay. */
  updatedAt: number;
}

/** What `spaces.setSettings` takes: the whole setting (it replaces the stored one). */
export type SpacesSettingsInput = Pick<SpacesSettings, "defaultSpace" | "fallback" | "fallbackOn">;

/** One computer in the order the relay tried (`cua_sdk::DefaultSpaceStep`). */
export interface DefaultSpaceStep {
  space: string;
  name: string;
  online: boolean;
  /** `default`, `fallback` or `here`. */
  role: "default" | "fallback" | "here" | string;
}

/** Why `space` was chosen. */
export type DefaultSpaceReason = "default" | "fallback" | "here" | "unset";

/** The relay's resolution (`cua_sdk::DefaultSpace`, camelCase). */
export interface DefaultSpace {
  /** Where a new task runs; null with `here`: on this computer. */
  space: string | null;
  reason: DefaultSpaceReason | string;
  defaultSpace: string | null;
  /** Null when no default is set. */
  defaultOnline: boolean | null;
  order: DefaultSpaceStep[];
}

export interface SpacesSettingsOps {
  "spaces.settings": { args: Record<string, never>; result: SpacesSettings | null };
  "spaces.setSettings": { args: SpacesSettingsInput; result: SpacesSettings | null };
  "spaces.resolveDefault": { args: Record<string, never>; result: DefaultSpace | null };
}

export const SPACES_SETTINGS_OPERATIONS = [
  "spaces.settings",
  "spaces.setSettings",
  "spaces.resolveDefault",
] as const satisfies readonly (keyof SpacesSettingsOps)[];

/** The native hosts' methods: the operations' own names and arguments. */
export const SPACES_SETTINGS_WEBKIT_METHODS = SPACES_SETTINGS_OPERATIONS;

const tauri = { tauri: "the Tauri app has no default Space" };

export const SPACES_SETTINGS_COVERAGE = {
  "spaces.settings": { webkit: { methods: ["spaces.settings"] }, tauri: [], unsupported: tauri },
  "spaces.setSettings": { webkit: { methods: ["spaces.setSettings"] }, tauri: [], unsupported: tauri },
  "spaces.resolveDefault": { webkit: { methods: ["spaces.resolveDefault"] }, tauri: [], unsupported: tauri },
} as const satisfies Record<keyof SpacesSettingsOps, OpCoverage>;

/** Arguments that take each down its usual path (coverage tests). */
export const SPACES_SETTINGS_ARGS = {
  "spaces.settings": {},
  "spaces.setSettings": { defaultSpace: "relay:m-studio", fallback: [], fallbackOn: true },
  "spaces.resolveDefault": {},
};

/** A computer the resolution may pick, in the account's order (oldest first). */
export interface DefaultCandidate {
  /** Its Space id (`relay:<machine>`, or a Space on one: `relay:<machine>/<name>`). */
  space: string;
  name: string;
  online: boolean;
  /** This computer (the caller's own): always last, the last resort. */
  here?: boolean;
}

/**
 * The relay's resolution rule (`GET /v1/spaces/default`), for a host or a
 * demo that has no relay to ask: the default, then the fallback list
 * (explicit, or every other computer when empty; never `here` or the
 * default again), then this computer. The first online one wins. It never
 * makes anything: with nothing online, the answer is this computer. A
 * default that isn't among the candidates any more (the computer was
 * removed) is treated as unset.
 */
export function resolveDefaultSpace(settings: SpacesSettingsInput | null, candidates: readonly DefaultCandidate[]): DefaultSpace {
  const here = candidates.find((c) => c.here) ?? null;
  const known = new Map(candidates.map((c) => [c.space, c]));
  const def = settings?.defaultSpace ? (known.get(settings.defaultSpace) ?? null) : null;
  const order: DefaultSpaceStep[] = [];
  if (def && !def.here) order.push({ space: def.space, name: def.name, online: def.online, role: "default" });
  if (!settings || settings.fallbackOn) {
    const explicit = settings?.fallback.length ? settings.fallback.map((id) => known.get(id)).filter((c): c is DefaultCandidate => Boolean(c)) : candidates;
    for (const c of explicit) {
      if (c.here || c.space === def?.space || order.some((o) => o.space === c.space)) continue;
      order.push({ space: c.space, name: c.name, online: c.online, role: "fallback" });
    }
  }
  if (here) order.push({ space: here.space, name: here.name, online: true, role: "here" });
  const first = order.find((o) => o.online && o.role !== "here");
  // A saved default that is no longer among the computers (removed since) counts as unset.
  const defaultSpace = def ? (settings?.defaultSpace ?? null) : null;
  const defaultOnline = defaultSpace === null ? null : def?.here ? true : (def?.online ?? false);
  if (def?.here) return { space: here!.space, reason: "default", defaultSpace, defaultOnline, order };
  if (!first)
    return {
      space: here?.space ?? null,
      reason: defaultSpace === null && order.length <= (here ? 1 : 0) ? "unset" : "here",
      defaultSpace,
      defaultOnline,
      order,
    };
  return {
    space: first.space,
    reason: first.role === "default" ? "default" : defaultSpace === null ? "unset" : "fallback",
    defaultSpace,
    defaultOnline,
    order,
  };
}

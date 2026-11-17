// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's default Space (ops/spaces-settings.ts): kept in memory,
 * resolved over the demo's machines the way the relay resolves it. Signed
 * out: null, as the native hosts answer. */

import type { MachineRow } from "../../contracts/host";
import { resolveDefaultSpace, type SpacesSettings } from "../../ops/spaces-settings";
import { HostError } from "../../adapter";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoSpacesSettingsState {
  spacesSettings: SpacesSettings;
}

export const demoSpacesSettingsState = (): DemoSpacesSettingsState => ({
  spacesSettings: { defaultSpace: null, fallback: [], fallbackOn: true, updatedAt: 0 },
});

/** A demo machine's Space id: this Mac's own is `relay:this-mac`. */
const spaceOf = (m: Pick<MachineRow, "id">) => `relay:${m.id}`;

export function demoSpacesSettingsHandlers(
  { state, now }: DemoContext,
  machines: () => MachineRow[],
): DemoHandlers<"spaces.settings" | "spaces.setSettings" | "spaces.resolveDefault"> {
  const copy = (): SpacesSettings => ({ ...state.spacesSettings, fallback: [...state.spacesSettings.fallback] });
  return {
    "spaces.settings": () => (state.identity ? copy() : null),
    "spaces.setSettings": ({ defaultSpace, fallback, fallbackOn }) => {
      if (!state.identity) return null;
      const ids = [defaultSpace, ...fallback].filter((x): x is string => x !== null);
      if (ids.some((id) => !/^relay:[^/]+(\/[a-z0-9-]{1,63})?$/.test(id))) throw new HostError("That isn't a Space id.", "bad_args");
      if (new Set(fallback).size !== fallback.length || fallback.length > 16)
        throw new HostError("The order to try lists a computer twice, or more than 16.", "bad_args");
      state.spacesSettings = { defaultSpace, fallback: [...fallback], fallbackOn, updatedAt: Math.floor(now() / 1000) };
      return copy();
    },
    "spaces.resolveDefault": () => {
      if (!state.identity) return null;
      const rows = machines().filter((m) => !m.device && m.via !== "direct");
      const candidates = rows.map((m) => ({ space: spaceOf(m), name: m.name, online: m.online, here: m.current === true }));
      // A Space made on one of them can be the default too (`relay:<machine>/<name>`): online while its computer is.
      const def = state.spacesSettings.defaultSpace;
      const onMachine = def?.match(/^relay:([^/]+)\/(.+)$/);
      const host = onMachine ? rows.find((m) => m.id === onMachine[1]) : undefined;
      if (def && onMachine && host) candidates.unshift({ space: def, name: onMachine[2]!, online: host.online, here: false });
      return resolveDefaultSpace(state.spacesSettings, candidates);
    },
  };
}

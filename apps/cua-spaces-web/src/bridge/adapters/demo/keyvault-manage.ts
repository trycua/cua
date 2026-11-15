// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's Keyvault management: a copy live in the design-review
 * Space (its "Signed in" badge and Access row), Wipe and Dismiss on it,
 * Delete with its live copy, and the revoked rules. */

import type { KeyvaultOverview, KvDelivery } from "../../contracts/keyvault";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoKeyvaultManageState {
  /** Delivered copies wiped (import ids). */
  wipedCopies: Set<string>;
  /** Delivered copies hidden from the notch. */
  dismissedCopies: string[];
}

export const demoKeyvaultManageState = (): DemoKeyvaultManageState => ({ wipedCopies: new Set(), dismissedCopies: [] });

/** The copy of the GitHub sign-in the demo's design-review Space holds. */
export const DEMO_COPY = "imp-github";

export function demoDelivery(now: number): KvDelivery {
  return {
    import_id: DEMO_COPY,
    target: "design-review",
    provider_id: "chrome",
    items: ["kv-gh-session", "kv-gh-sess"],
    caller_fp: "fp-claude",
    delivered_ms: now - 5 * 60_000,
    expires_ms: 0,
    wiped: false,
  };
}

/** The overview with this feature's copies and dismissals. */
export function withDemoCopies(o: KeyvaultOverview, state: DemoKeyvaultManageState, now: number): KeyvaultOverview {
  if (o.availability !== "ready") return o;
  return {
    ...o,
    deliveries: state.wipedCopies.has(DEMO_COPY) ? [] : [demoDelivery(now)],
    dismissed: [...state.dismissedCopies],
  };
}

export function demoKeyvaultManageHandlers(
  { state, wait, emit, stepMs }: DemoContext,
  overview: () => KeyvaultOverview,
): DemoHandlers<"keyvault.showItems" | "keyvault.delete" | "keyvault.run" | "keyvault.dismiss"> {
  return {
    "keyvault.showItems": async () => {
      await wait(stepMs);
      return overview();
    },
    "keyvault.delete": async ({ itemIds }) => {
      await wait(stepMs);
      const gone = new Set(itemIds);
      state.items = state.items.filter((i) => !gone.has(i.id));
      // Live copies of what was deleted are wiped with it.
      if (["kv-gh-session", "kv-gh-sess"].some((id) => gone.has(id))) state.wipedCopies.add(DEMO_COPY);
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.run": async ({ command }) => {
      await wait(stepMs);
      if (command.type === "revoke-grant") state.revokedGrants.add(command.id);
      else if (command.type === "release") state.wipedCopies.add(DEMO_COPY);
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.dismiss": ({ imports }) => {
      state.dismissedCopies = [...state.dismissedCopies, ...imports.filter((i) => !state.dismissedCopies.includes(i))];
      emit({ type: "keyvault.changed" });
      return null;
    },
  };
}

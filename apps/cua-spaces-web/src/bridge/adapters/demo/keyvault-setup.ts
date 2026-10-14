// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's Keyvault setup: `?demo=novault` starts with no vault;
 * "Set up Keyvault" creates it and answers a sample recovery key. */

import type { KeyvaultOverview } from "../../contracts/keyvault";
import { demoKeyvaultOverview } from "../demo-data";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoKeyvaultSetupState {
  /** The vault exists (false: the "No Keyvault yet" page). */
  keyvaultSetUp: boolean;
}

export const demoKeyvaultSetupState = (setUp: boolean): DemoKeyvaultSetupState => ({ keyvaultSetUp: setUp });

export const DEMO_RECOVERY_KEY = "DEMO-7K2Q-RX4M-9TPA-W3HC";

/** The broker's answer before setup: no vault, Touch ID and a passphrase available. */
export function demoNoVaultOverview(now: number): KeyvaultOverview {
  const o = demoKeyvaultOverview(now, [], true);
  return { ...o, availability: "no_vault", message: "No Keyvault yet.", itemsTotal: 0, status: o.status && { ...o.status, initialized: false, items: 0, unlock_protectors: [] } };
}

export function demoKeyvaultSetupHandlers({ state, wait, emit, stepMs }: DemoContext): DemoHandlers<"keyvault.setup"> {
  return {
    "keyvault.setup": async () => {
      await wait(stepMs);
      state.keyvaultSetUp = true;
      emit({ type: "keyvault.changed" });
      return { recoveryKey: DEMO_RECOVERY_KEY };
    },
  };
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The parity replay for Settings → Agents (`agent-keys`): the core calls the
 * section makes go through the bridge's own functions (`agent-keys.ts`), and
 * the states they reach are recorded for the Playwright harness to put on
 * the real screen (`window.__cuaParity.showAgentKeys`). See `parity.ts`.
 */

import {
  agentKeyForm,
  agentKeyNameProblemOf,
  agentKeyRemoveConfirm,
  agentKeysStore,
  agentKeysView,
  type AgentKeyConfirm,
  type AgentKeyFormInput,
  type AgentKeyFormView,
  type AgentKeysInput,
  type AgentKeysView,
} from "./agent-keys";
import type { CoreClient } from "./core";
import type { AgentKeyProvider } from "./ops/agent-keys";
import type { BridgeStore } from "./store";

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

export type AgentKeysCheckpoint =
  | { kind: "agent-keys"; input: AgentKeysInput; view: AgentKeysView }
  | { kind: "agent-key-form"; input: AgentKeysInput; form: AgentKeyFormInput; view: AgentKeyFormView }
  | { kind: "agent-key-remove"; input: AgentKeysInput; env: string; confirm: AgentKeyConfirm | null };

export function agentKeysParityMethods(core: CoreClient, record: (c: AgentKeysCheckpoint) => void): Record<string, (a: Args) => unknown> {
  return {
    "agentKeys.view": (a) => {
      // The goldens are the Mac's words.
      const view = agentKeysView(core, a.input, a.hostOs ?? "macos")!;
      record({ kind: "agent-keys", input: a.input, view });
      return view;
    },
    "agentKeys.form": (a) => {
      const view = agentKeyForm(core, a.input, a.form, a.hostOs ?? "macos")!;
      record({ kind: "agent-key-form", input: a.input, form: a.form, view });
      return view;
    },
    "agentKeys.removeConfirm": (a) => {
      const confirm = agentKeyRemoveConfirm(core, a.input, a.env);
      record({ kind: "agent-key-remove", input: a.input, env: a.env, confirm });
      return confirm;
    },
    "agentKeys.nameProblem": (a) => agentKeyNameProblemOf(core, a.name),
  };
}

export interface AgentKeysParityHandle {
  /** Settings → Agents for this input, with a sheet or a Remove question open. */
  showAgentKeys(input: AgentKeysInput, open?: { sheet?: { provider: AgentKeyProvider; env: string | null } | null; removing?: string | null }): void;
}

export const agentKeysParityHandle = (store: BridgeStore): AgentKeysParityHandle => ({
  showAgentKeys: (input, open) => agentKeysStore(store.adapter).show(input, open),
});

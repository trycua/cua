// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Agents: `useAgentKeys()` over `ops/agent-keys.ts`, with every
 * word and rule from the app core's `agentKeys.*` (`agent_keys.rs`): the
 * rows, the add or replace sheet, which variable names can hold a key and
 * the question before removing one.
 *
 * Holds only what the host answers (provider, variable, last four
 * characters, when it was added) and which sheet is open. A key the page
 * sends is never kept here: the sheet's field holds it until Save.
 */

import { useContext, useEffect, useSyncExternalStore } from "react";
import { isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import { hostOs, type HostOs } from "./host-os";
import type { AgentKeyProvider, AgentKeysReport } from "./ops/agent-keys";

/* ---- The core's words ------------------------------------------------------------ */

/** What the section shows (`agent_keys::AgentKeysInput`). */
export interface AgentKeysInput {
  keys: { provider: string; env: string; last4: string; addedMs: number }[];
  unavailable?: string | null;
  error?: string | null;
}

export interface AgentKeyRowView {
  env: string;
  provider: AgentKeyProvider;
  title: string;
  detail: string;
  set: boolean;
  status: string;
  addedMs: number | null;
  actionLabel: string;
  removeLabel: string | null;
}

export interface AgentKeysView {
  title: string;
  intro: string;
  rows: AgentKeyRowView[];
  addOtherLabel: string;
  otherHelp: string;
  notice: string | null;
  canEdit: boolean;
  addedLabel: string;
}

/** What the sheet is for (`agent_keys::AgentKeyFormInput`). */
export interface AgentKeyFormInput {
  provider: AgentKeyProvider;
  env?: string | null;
  name?: string;
  hasValue?: boolean;
}

export interface AgentKeyFormView {
  title: string;
  lede: string;
  nameLabel: string | null;
  namePlaceholder: string | null;
  nameError: string | null;
  valueLabel: string;
  valuePlaceholder: string;
  valueHelp: string;
  saveLabel: string;
  cancelLabel: string;
  canSave: boolean;
  provider: AgentKeyProvider;
  env: string | null;
}

export interface AgentKeyConfirm {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
}

export const agentKeysInputOf = (report: AgentKeysReport | undefined, error: Error | null): AgentKeysInput => ({
  keys: (report?.keys ?? []).map(({ provider, env, last4, addedMs }) => ({ provider, env, last4, addedMs })),
  unavailable: report?.available === false ? (report.unavailable ?? "this machine can't keep keys") : null,
  error: error ? error.message : null,
});

type Json = Record<string, unknown>;

/** The section (null without the core: the page says so), in the words of the system the app
 * runs on (`os`: where the keys stay is that system's). */
export const agentKeysView = (core: CoreClient, input: AgentKeysInput, os: HostOs = hostOs()): AgentKeysView | null =>
  core.tryCall<AgentKeysView>("agentKeys.view", { input: input as unknown as Json, hostOs: os }) ?? null;

/** The add or replace sheet. */
export const agentKeyForm = (core: CoreClient, input: AgentKeysInput, form: AgentKeyFormInput, os: HostOs = hostOs()): AgentKeyFormView | null =>
  core.tryCall<AgentKeyFormView>("agentKeys.form", { input: input as unknown as Json, form: form as unknown as Json, hostOs: os }) ?? null;

/** The question before removing `env` (null: none is saved). */
export const agentKeyRemoveConfirm = (core: CoreClient, input: AgentKeysInput, env: string): AgentKeyConfirm | null =>
  core.tryCall<AgentKeyConfirm | null>("agentKeys.removeConfirm", { input: input as unknown as Json, env }) ?? null;

/** Why `name` can't hold an Other key (null: it can). */
export const agentKeyNameProblemOf = (core: CoreClient, name: string): string | null =>
  core.tryCall<string | null>("agentKeys.nameProblem", { name }) ?? null;

/* ---- The store ------------------------------------------------------------------- */

export interface AgentKeysState {
  report: AgentKeysReport | undefined;
  error: Error | null;
  unsupported: boolean;
  /** The add or replace sheet, when open. */
  sheet: { provider: AgentKeyProvider; env: string | null } | null;
  /** The key a Remove asks about, when asked. */
  removing: string | null;
}

const INITIAL: AgentKeysState = { report: undefined, error: null, unsupported: false, sheet: null, removing: null };
const asError = (e: unknown) => (e instanceof Error ? e : new Error(String(e)));

/** What the store listens to for "the user is back": the window and its document. */
export interface FocusTarget {
  addEventListener(type: "focus", listener: () => void): void;
  removeEventListener(type: "focus", listener: () => void): void;
  document?: {
    visibilityState?: string;
    addEventListener(type: "visibilitychange", listener: () => void): void;
    removeEventListener(type: "visibilitychange", listener: () => void): void;
  };
}

export class AgentKeysStore {
  private state: AgentKeysState = INITIAL;
  private listeners = new Set<() => void>();
  private loaded = false;
  /** A parity checkpoint on screen: host answers don't replace it. */
  private pinned = false;
  /** Bumps whenever a save or a remove starts or ends: a list read from before it must not replace the shown one. */
  private epoch = 0;
  private reading: { epoch: number; done: Promise<void> } | null = null;

  constructor(readonly adapter: DataAdapter) {}

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };
  get = () => this.state;

  private set(patch: Partial<AgentKeysState>): void {
    this.state = { ...this.state, ...patch };
    for (const l of [...this.listeners]) l();
  }

  ensure(): void {
    if (this.loaded) return;
    this.loaded = true;
    void this.refresh();
  }

  /** Reads the list again. Keys also change outside the app (`cua agent keys`), so the page asks
   * whenever it comes into view (`watch`). One read at a time, and never over a newer save or remove. */
  refresh(): Promise<void> {
    if (this.reading && this.reading.epoch === this.epoch) return this.reading.done;
    const epoch = this.epoch;
    const done: Promise<void> = this.read(epoch).finally(() => {
      if (this.reading?.done === done) this.reading = null;
    });
    this.reading = { epoch, done };
    return done;
  }

  private async read(epoch: number): Promise<void> {
    try {
      const report = await this.adapter.call("agentKeys.list", {});
      if (!this.pinned && epoch === this.epoch) this.set({ report, error: null });
    } catch (e) {
      if (this.pinned || epoch !== this.epoch) return;
      if (isUnsupported(e)) this.set({ unsupported: true });
      else this.set({ error: asError(e) });
    }
  }

  /** Reads the list now, and again whenever the window gets focus, the document becomes visible
   * or the host says agents changed. Returns the stop. */
  watch(target: FocusTarget | undefined = typeof window === "undefined" ? undefined : window): () => void {
    this.loaded = true;
    const again = () => void this.refresh();
    const onVisible = () => {
      if (target?.document?.visibilityState !== "hidden") again();
    };
    target?.addEventListener("focus", again);
    target?.document?.addEventListener("visibilitychange", onVisible);
    const unsubscribe = this.adapter.subscribe((e) => {
      if (e.type === "agents.changed") again();
    });
    again();
    return () => {
      target?.removeEventListener("focus", again);
      target?.document?.removeEventListener("visibilitychange", onVisible);
      unsubscribe();
    };
  }

  /** Saves a key; the list after it replaces the shown one. Throws the host's words. */
  async save(provider: AgentKeyProvider, value: string, env: string | null): Promise<void> {
    this.epoch++;
    const report = await this.adapter.call("agentKeys.set", { provider, value, env });
    this.epoch++;
    this.pinned = false;
    this.set({ report, error: null, sheet: null });
  }

  async remove(env: string): Promise<void> {
    this.epoch++;
    const report = await this.adapter.call("agentKeys.remove", { env });
    this.epoch++;
    this.pinned = false;
    this.set({ report, error: null, removing: null });
  }

  openSheet(provider: AgentKeyProvider, env: string | null = null): void {
    this.set({ sheet: { provider, env }, removing: null });
  }
  closeSheet(): void {
    this.set({ sheet: null });
  }
  askRemove(env: string | null): void {
    this.set({ removing: env, sheet: null });
  }

  /** Parity: shows this input, and a sheet or a Remove question over it. */
  show(input: AgentKeysInput, open?: { sheet?: { provider: AgentKeyProvider; env: string | null } | null; removing?: string | null }): void {
    this.pinned = true;
    this.loaded = true;
    const report: AgentKeysReport = {
      keys: input.keys.map((k) => ({ ...k, provider: k.provider === "anthropic" || k.provider === "openai" ? k.provider : "other" })),
      providers: [],
      available: !input.unavailable,
      unavailable: input.unavailable ?? null,
    };
    this.set({ report, error: input.error ? new Error(input.error) : null, unsupported: false, sheet: open?.sheet ?? null, removing: open?.removing ?? null });
  }
}

const stores = new WeakMap<DataAdapter, AgentKeysStore>();

/** The adapter's one Settings → Agents store. */
export function agentKeysStore(adapter: DataAdapter): AgentKeysStore {
  let s = stores.get(adapter);
  if (!s) stores.set(adapter, (s = new AgentKeysStore(adapter)));
  return s;
}

/* ---- The hook -------------------------------------------------------------------- */

export interface AgentKeysHook extends AgentKeysState {
  isLoading: boolean;
  input: AgentKeysInput;
  /** The core's section (null when the core isn't loaded). */
  view: AgentKeysView | null;
  core: CoreClient;
  refresh(): Promise<void>;
  /** Adds or replaces a key; `env` names an Other key. */
  save(provider: AgentKeyProvider, value: string, env: string | null): Promise<void>;
  remove(env: string): Promise<void>;
  openSheet(provider: AgentKeyProvider, env?: string | null): void;
  closeSheet(): void;
  askRemove(env: string | null): void;
  /** The question before removing `env` (null: none is saved). */
  removeConfirmOf(env: string): AgentKeyConfirm | null;
}

const noopSubscribe = () => () => {};

/** `watch`: read the list again whenever the page is in front (mounted), the window gets focus
 * or the document becomes visible, so a key changed with the CLI shows up. */
export function useAgentKeys({ watch = false }: { watch?: boolean } = {}): AgentKeysHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const store = ctx.adapter ? agentKeysStore(ctx.adapter) : null;
  store?.ensure();
  useEffect(() => (watch && store ? store.watch() : undefined), [watch, store]);
  const state = useSyncExternalStore(store ? store.subscribe : noopSubscribe, () => (store ? store.get() : INITIAL), () => INITIAL);
  const input = agentKeysInputOf(state.report, state.error);
  const need = () => {
    if (!store) throw new Error("The app isn't connected yet");
    return store;
  };
  return {
    ...state,
    isLoading: !state.report && !state.error && !state.unsupported,
    input,
    view: agentKeysView(ctx.core, input),
    core: ctx.core,
    refresh: () => (store ? store.refresh() : Promise.resolve()),
    save: (provider, value, env) => need().save(provider, value, env),
    remove: (env) => need().remove(env),
    openSheet: (provider, env = null) => store?.openSheet(provider, env),
    closeSheet: () => store?.closeSheet(),
    askRemove: (env) => store?.askRemove(env),
    removeConfirmOf: (env) => agentKeyRemoveConfirm(ctx.core, input, env),
  };
}

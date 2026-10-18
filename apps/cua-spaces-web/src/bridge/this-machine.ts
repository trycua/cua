// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * This machine: its page (`host.panel`) and the host setup form
 * (`host.formInitial`, `.formReduce`, `.formView`), the core calls the
 * SwiftUI app's `ThisMachineView` and `HostModel` make. The form's submit
 * sends the request the core built (`host.setUp`); the page's buttons run
 * `host.action`. Both refresh the machines list after.
 */

import { useContext, useSyncExternalStore } from "react";
import { HostError, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { HostActionId, HostFormView as HostFormViewBase } from "./contracts/host";
import type { HostFormAction, HostFormState, HostRunAction, HostSetupRequest } from "./ops/host-setup";

export type { HostFormAction, HostFormState, HostRunAction, HostSetupRequest } from "./ops/host-setup";

/** The form as drawn, with the request host setup receives once it is valid. */
export interface HostFormView extends HostFormViewBase {
  request?: HostSetupRequest | null;
}

/* ---- The core's decisions ------------------------------------------------------ */

const FALLBACK_FORM: HostFormState = {
  name: "",
  allow: "",
  advanced: false,
  direct: false,
  listen: "0.0.0.0:3211",
  relayUrl: "https://relay.cua.ai",
  busy: false,
  error: null,
  spare: false,
};

export function hostFormInitial(core: CoreClient): HostFormState {
  return core.tryCall<HostFormState>("host.formInitial", {}) ?? { ...FALLBACK_FORM };
}

export function hostFormReduce(core: CoreClient, state: HostFormState, action: HostFormAction): HostFormState {
  return core.tryCall<HostFormState>("host.formReduce", { state, action }) ?? state;
}

/** The form as drawn for the signed-in `identity`; null without the core. */
export function hostFormView(core: CoreClient, state: HostFormState, identity: string | null): HostFormView | null {
  return core.tryCall<HostFormView>("host.formView", { state, identity }) ?? null;
}

/* ---- The form and the buttons ---------------------------------------------------- */

/** A failed setup or button, as the host words it (the SwiftUI app's
 * `HostSetupFailure`): a short title, what to do, and the raw error for
 * Details. A host that only says the error leaves `title` out. */
export interface HostFailure {
  title?: string;
  message: string;
  /** The raw error, unchanged. */
  details?: string;
  /** The retry button: "Retry", or "Sign In" when the account is missing. */
  actionLabel?: string;
}

export interface ThisMachineState {
  /** The setup form's state; null while the page shows. */
  form: HostFormState | null;
  /** A button is running. */
  busy: boolean;
  /** The last button that failed, and why; Retry runs it again. */
  error: HostFailure | null;
  /** The last setup that failed, and why; Retry sends the same request. */
  setupError: HostFailure | null;
  /** The parity harness's signed-in identity for the form. */
  identity?: string | null;
}

const IDLE: ThisMachineState = { form: null, busy: false, error: null, setupError: null };

/** A thrown error as the page shows it. */
export function hostFailure(e: unknown): HostFailure {
  const message = e instanceof Error ? e.message : String(e);
  const p = e instanceof HostError ? e.presented : undefined;
  return p ? { title: p.title, message, details: p.details, actionLabel: p.actionLabel } : { message };
}

export class ThisMachineStore {
  private state: ThisMachineState = IDLE;
  private listeners = new Set<() => void>();
  /** What Retry sends again: the failed setup's request, the failed button. */
  private failedRequest: HostSetupRequest | null = null;
  private failedAction: HostRunAction | null = null;

  constructor(
    private readonly adapter: DataAdapter,
    private readonly core: CoreClient,
    /** The host changed: refetch the machines. */
    private readonly changed: () => Promise<void>,
  ) {}

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  get = (): ThisMachineState => this.state;

  private set(next: Partial<ThisMachineState>): void {
    this.state = { ...this.state, ...next };
    for (const l of [...this.listeners]) l();
  }

  /** Opens the form, with a setup choice's profile (`desktop`, `spare`). */
  openForm(profile?: string): void {
    let form = hostFormInitial(this.core);
    if (profile) form = hostFormReduce(this.core, form, { type: "set-profile", profile });
    this.failedRequest = null;
    this.set({ form, error: null, setupError: null });
  }

  closeForm(): void {
    this.failedRequest = null;
    this.set({ form: null, setupError: null });
  }

  send(action: HostFormAction): void {
    if (this.state.form) this.set({ form: hostFormReduce(this.core, this.state.form, action) });
  }

  /** Submits the request the core validated; the form closes when setup
   * worked. Resolves to whether it did. */
  async submit(identity: string | null): Promise<boolean> {
    const before = this.state.form;
    if (!before) return false;
    const request = hostFormView(this.core, before, identity)?.request;
    if (!request) return false;
    return this.setUp(request);
  }

  /** Retry on a failed setup: the same request again (signing in first
   * when that is what failed; the host does it inline). */
  retrySetUp(): Promise<boolean> {
    return this.failedRequest ? this.setUp(this.failedRequest) : Promise.resolve(false);
  }

  private async setUp(request: HostSetupRequest): Promise<boolean> {
    if (this.state.form?.busy) return false;
    this.send({ type: "submit" });
    try {
      await this.adapter.call("host.setUp", { request });
      this.failedRequest = null;
      this.set({ form: null, setupError: null });
      await this.changed();
      return true;
    } catch (e) {
      // The failure stays while a retry runs, so Retry can show progress.
      this.failedRequest = request;
      const failure = hostFailure(e);
      this.set({ setupError: failure });
      this.send({ type: "failed", error: failure.details ?? failure.message });
      return false;
    }
  }

  /** Runs a page button (Stop sharing, Remove, a switch, Sign In). `set-up` opens the form. */
  async run(action: HostActionId): Promise<void> {
    if (action === "set-up") return this.openForm();
    if (this.state.busy) return;
    this.set({ busy: true });
    try {
      await this.adapter.call("host.action", { action: action as HostRunAction });
      this.failedAction = null;
      this.set({ error: null });
    } catch (e) {
      this.failedAction = action as HostRunAction;
      this.set({ error: hostFailure(e) });
    } finally {
      this.set({ busy: false });
      await this.changed();
    }
  }

  /** Retry on a failed button: the same button again. */
  retry(): Promise<void> {
    return this.failedAction ? this.run(this.failedAction) : Promise.resolve();
  }

  openSettings(url: string): Promise<null> {
    return this.adapter.call("host.openSettings", { url });
  }

  /** Parity harness: this form state on screen, for `identity`. */
  show(form: HostFormState | null, identity: string | null): void {
    this.set({ ...IDLE, form, identity });
  }
}

/* ---- Hook ------------------------------------------------------------------------------ */

export interface ThisMachineHook extends ThisMachineState {
  /** The form as the core draws it; null while the page shows. */
  formView: HostFormView | null;
  openForm(profile?: string): void;
  closeForm(): void;
  send(action: HostFormAction): void;
  /** Resolves to whether setup worked. */
  submit(): Promise<boolean>;
  /** Sends the failed setup's request again. */
  retrySetUp(): Promise<boolean>;
  run(action: HostActionId): Promise<void>;
  /** Runs the failed button again. */
  retry(): Promise<void>;
  openSettings(url: string): Promise<void>;
}

const noopSubscribe = () => () => {};

/** The setup form and the page's buttons. `identity` is who is signed in
 * (the form says which account the machine joins). The page itself is
 * `useMachines()`'s current machine `panel`. */
export function useThisMachine(identity: string | null): ThisMachineHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const tm = ctx.store?.thisMachine;
  const state = useSyncExternalStore(tm ? tm.subscribe : noopSubscribe, tm ? tm.get : () => IDLE, () => IDLE);
  const who = state.identity !== undefined ? state.identity : identity;
  return {
    ...state,
    formView: state.form ? hostFormView(ctx.core, state.form, who) : null,
    openForm: (p) => tm?.openForm(p),
    closeForm: () => tm?.closeForm(),
    send: (a) => tm?.send(a),
    submit: () => tm?.submit(who) ?? Promise.resolve(false),
    retrySetUp: () => tm?.retrySetUp() ?? Promise.resolve(false),
    run: (a) => tm?.run(a) ?? Promise.resolve(),
    retry: () => tm?.retry() ?? Promise.resolve(),
    openSettings: async (url) => {
      await tm?.openSettings(url);
    },
  };
}

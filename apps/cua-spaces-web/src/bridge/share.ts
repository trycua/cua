// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Share sheet for one Space, as the SwiftUI app's `ShareModel` runs it:
 * the app core's `share.*` state machine over the host's sharing commands
 * (`ops/share.ts`). Every word on the sheet is the core's. Sharing asks for
 * presence in the daemon before anything reaches the relay; the sheet only
 * shows that a request is running.
 */

import { useContext, useSyncExternalStore } from "react";
import type { DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { ShareEntry, ShareInput, ShareSheetAction, ShareSheetState, ShareSheetView } from "./contracts/share";
import type { Space } from "./contracts/spaces";
import type { BridgeStore } from "./store";
import { telemetryShare, type TelemetrySignal } from "./telemetry";

export const shareInitial = (core: CoreClient) => core.call<ShareSheetState>("share.initial", {});
export const shareReduce = (core: CoreClient, input: ShareInput, state: ShareSheetState, action: ShareSheetAction) =>
  core.call<ShareSheetState>("share.reduce", { input, state, action });
export const shareView = (core: CoreClient, input: ShareInput, state: ShareSheetState) =>
  core.call<ShareSheetView>("share.view", { input, state });

/** The Space can be shared: a relay host's, or a driver with `relay_attach`
 * (as the SwiftUI app decides it). */
export const shareable = (space: Pick<Space, "id" | "sdk">): boolean =>
  space.id.startsWith("relay:") || Boolean(space.sdk?.features.includes("relay_attach"));

export interface ShareSession {
  input: ShareInput;
  state: ShareSheetState;
  view: ShareSheetView;
}

const words = (e: unknown) => (e instanceof Error ? e.message : String(e));

export class ShareStore {
  private session: { input: ShareInput; state: ShareSheetState } | null = null;
  private view: ShareSession | null = null;
  private listeners = new Set<() => void>();

  constructor(
    readonly adapter: DataAdapter,
    readonly core: CoreClient,
    /** Where usage events go (`BridgeStore.telemetry`): shared, unshared or
     * a role changed, view-only or not; never who. */
    private readonly track: (signals: TelemetrySignal[]) => void = () => {},
  ) {}

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };
  getSession = () => this.view;

  private publish(): void {
    const s = this.session;
    this.view = s ? { ...s, view: shareView(this.core, s.input, s.state) } : null;
    for (const l of [...this.listeners]) l();
  }

  private setShares(spaceId: string, shares: ShareEntry[]): void {
    if (this.session?.input.spaceId !== spaceId) return;
    this.session = { ...this.session, input: { ...this.session.input, shares } };
  }

  /** Opens the sheet and reads who the Space is shared with. */
  open(space: Pick<Space, "id" | "name" | "sdk">, signedIn: boolean): void {
    if (this.core.status !== "ready") throw new Error("Sharing needs the app core, which isn't loaded");
    const input: ShareInput = { spaceId: space.id, spaceName: space.name, shares: [], signedIn, shareable: shareable(space) };
    this.session = { input, state: shareInitial(this.core) };
    this.publish();
    if (!signedIn) return;
    void this.adapter.call("sharing.list", { spaceId: space.id }).then(
      (shares) => {
        this.setShares(space.id, shares);
        this.publish();
      },
      () => {},
    );
  }

  close(): void {
    this.session = null;
    this.publish();
  }

  /** One input through the core; a share or unshare it asks for runs on the host. */
  async send(action: ShareSheetAction): Promise<void> {
    const s = this.session;
    if (!s) return;
    const wasBusy = s.state.busy;
    const state = shareReduce(this.core, s.input, s.state, action);
    this.session = { ...s, state };
    this.publish();
    const request = state.request;
    if (wasBusy || !state.busy || !request) return;
    const spaceId = s.input.spaceId;
    // Who it was shared with before the call (a role change or a new
    // share), for the usage event (`ShareModel.send`).
    const before = s.input;
    let outcome: ShareSheetAction;
    try {
      const shares =
        request.kind === "share"
          ? await this.adapter.call("sharing.share", { spaceId: request.space, who: request.who, role: request.role })
          : await this.adapter.call("sharing.unshare", { spaceId: request.space, who: request.who });
      this.setShares(spaceId, shares);
      outcome = { type: "done" };
    } catch (e) {
      outcome = { type: "failed", error: words(e) };
    }
    this.track(telemetryShare(this.core, before, state, outcome));
    const now = this.session;
    if (now?.input.spaceId !== spaceId) return;
    this.session = { ...now, state: shareReduce(this.core, now.input, now.state, outcome) };
    this.publish();
  }

  /* ---- parity harness ---- */

  /** Draws this input and state, as if the host and the user had produced them. */
  show(input: ShareInput, state: ShareSheetState): void {
    this.session = { input, state };
    this.publish();
  }
}

const stores = new WeakMap<BridgeStore, ShareStore>();

export function shareStore(store: BridgeStore): ShareStore {
  let s = stores.get(store);
  if (!s) {
    s = new ShareStore(store.adapter, store.core, (signals) => void store.telemetry.track(signals));
    stores.set(store, s);
  }
  return s;
}

const noop = () => () => {};

export interface ShareHook {
  /** The open sheet, or null. */
  session: ShareSession | null;
  sharing: ShareStore | null;
  /** Opens the sheet for a Space. */
  open(space: Pick<Space, "id" | "name" | "sdk">, signedIn: boolean): Promise<void>;
}

/** "Share": the open sheet and its actions. */
export function useShareSheet(): ShareHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("useShareSheet needs a <BridgeProvider> above it");
  const s = ctx.store ? shareStore(ctx.store) : null;
  const session = useSyncExternalStore(s ? s.subscribe : noop, () => s?.getSession() ?? null, () => null);
  return { session, sharing: s, open: (space, signedIn) => ctx.ready.then((b) => shareStore(b).open(space, signedIn)) };
}

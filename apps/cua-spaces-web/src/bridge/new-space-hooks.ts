// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** New Space and "Connect a cloud" for components (see `new-space.ts`). */

import { useContext, useEffect, useState, useSyncExternalStore } from "react";
import { isUnsupported } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { CloudConnectAction, CloudConnectView, RuntimeSwitch, WizardAction, WizardEnv, WizardView } from "./contracts/new-space";
import type { Space, SpaceOs } from "./contracts/spaces";
import { useMachines, useSession, useSettings, useSpaces } from "./hooks";
import { newPendingId } from "./store";
import {
  CLOSED_NEW_SPACE,
  cloudConnectInitial,
  cloudConnectReduce,
  cloudConnectView,
  connectInput,
  firstSpaceOffers,
  loadNewSpaceHostData as loadHostData,
  optionsPending,
  readNewSpaceSession as read,
  subscribeNewSpaceSession as subscribe,
  updateNewSpaceSession as update,
  wizardCreateArgs,
  wizardEnv,
  wizardInitial,
  wizardOffered,
  wizardReduce,
  wizardView,
  type FirstSpaceOffer,
  type NewSpaceSession,
} from "./new-space";

function useNewSpaceEnv(): { core: CoreClient; env: WizardEnv; s: NewSpaceSession } {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const s = useSyncExternalStore(subscribe, read, read);
  const { data: settings } = useSettings();
  const { data: sessionData } = useSession();
  const { data: machines } = useMachines();
  const core = ctx.core;
  const env =
    s.pinnedEnv ??
    (core.status === "ready"
      ? wizardEnv(core, {
          options: s.options,
          clouds: s.clouds,
          defaultLocation: settings?.values.defaultLocation,
          cloudAvailable: Boolean(sessionData?.fleet.configured),
          machines,
        })
      : ({ defaultLocation: "local", cloudAvailable: false, localAvailable: true, maxCpus: 8 } as WizardEnv));
  return { core, env, s };
}

export interface NewSpaceWizardHook {
  /** The wizard runs on this host (else `show` starts the host's own create). */
  offered: boolean;
  open: boolean;
  /** Parity: drawn with a replay's env, not the live one. */
  pinned: boolean;
  env: WizardEnv;
  /** macOS VMs running on this machine, when the host says (Apple's limit). */
  macosVmsRunning: number | null;
  /** The core's view, while open. */
  view: WizardView | null;
  /** The address form's text, while open. */
  address: { url: string; token: string; name: string };
  /** Opens the wizard; `on` presets "Run on" (`host:<machine>`, a cloud);
   * `os` opens it on that system, on this machine (an empty home tile). */
  show(on?: string | null, os?: SpaceOs): void;
  close(): void;
  send(action: WizardAction): void;
  /** Starts the plan's create through the bridge's creates flow and closes:
   * its row's id (it lists at once, creating) and the create, which
   * settles when the Space is ready. */
  create(): { pendingId: string; done: Promise<Space> } | null;
  /** "Connect by address": the handshake; its error shows inline. */
  submitAddress(): Promise<void>;
  /** "Use built-in Lume" / "Use built-in runtime" (`view.runtimeSwitch`):
   * changes the setting as Settings, Runtimes does, then reads this
   * machine's runtimes again (it can run the Space now). */
  applyRuntimeSwitch(change: RuntimeSwitch): Promise<void>;
}

/** The Settings, Runtimes row a New Space runtime switch changes. */
export const RUNTIME_SWITCH_ROW: Record<string, string> = { "runtime.lume": "macos-runtime", "runtime.linux": "linux-runtime" };

/** New Space: the core's wizard over the bridge's data. */
export function useNewSpaceWizard(): NewSpaceWizardHook {
  const ctx = useContext(BridgeContext)!;
  const { core, env, s } = useNewSpaceEnv();
  const { createSpace, refresh } = useSpaces();
  const offered = wizardOffered(core, ctx.mode);
  const view = s.open && s.state && core.status === "ready" ? wizardView(core, s.state, env) : null;
  const send = (action: WizardAction) => update((x) => (x.state ? { ...x, state: wizardReduce(core, x.state, action, env) } : x));

  return {
    offered,
    open: s.open,
    pinned: s.pinnedEnv !== null,
    env,
    macosVmsRunning: s.options?.macosVmsRunning ?? null,
    view,
    address: (s.state?.address as { url: string; token: string; name: string } | undefined) ?? { url: "", token: "", name: "" },
    show(on, os) {
      if (!offered) return;
      const local = os ? { ...env, defaultLocation: "local" as const } : env;
      let state = wizardInitial(core, local);
      if (os) state = wizardReduce(core, state, { type: "choose-os", os }, local);
      update((x) => ({ ...x, open: true, pinnedEnv: null, state, placeOn: on ?? null }));
      void ctx.ready.then(loadHostData);
    },
    close: () => update((x) => ({ ...x, open: false, state: null, pinnedEnv: null, placeOn: null })),
    send,
    create() {
      if (!view) return null;
      const plan = view.plan;
      const args = wizardCreateArgs(core, plan);
      update((x) => ({ ...x, open: false, state: null, pinnedEnv: null }));
      const pendingId = newPendingId();
      return { pendingId, done: createSpace({ ...args, os: plan.image.os }, pendingId) };
    },
    async submitAddress() {
      const call = view?.address.submit;
      if (!call || !ctx.adapter) return;
      send({ type: "submit-address" });
      try {
        await ctx.adapter.call("spaces.add", { url: call.url, token: call.token, name: call.name });
        update((x) => ({ ...x, open: false, state: null, pinnedEnv: null }));
        await refresh();
      } catch (e) {
        send({ type: "address-failed", error: e instanceof Error ? e.message : String(e) });
      }
    },
    async applyRuntimeSwitch(change) {
      const row = RUNTIME_SWITCH_ROW[change.setting];
      if (!row) return;
      const store = await ctx.ready;
      await store.chooseSetting(row, change.value);
      await loadHostData(store);
    },
  };
}

export interface FirstSpaceHook {
  /** Linux first, then macOS; empty while the core or the host's options load. */
  offers: FirstSpaceOffer[];
  /** macOS VMs running on this machine, when the host says (Apple's limit). */
  macosVmsRunning: number | null;
}

/** How often, and how many times, the first-Space offer asks again for
 * options the host did not give. */
export const FIRST_SPACE_RETRY_MS = 5_000;
const FIRST_SPACE_RETRIES = 6;

/** What this machine can create first (the wizard's defaults on it), and
 * the host's options asked for while the Spaces list is empty. */
export function useFirstSpaceOffers(): FirstSpaceHook {
  const ctx = useContext(BridgeContext)!;
  const { core, env, s } = useNewSpaceEnv();
  const offered = wizardOffered(core, ctx.mode);
  const [tries, setTries] = useState(0);
  // The host's options carry this Mac's runtimes, storage and what is
  // pulled. (`ready` is replaced once the host is up: wait on the live one.)
  // Options the host did not give in time (it was still starting, or a
  // signed-out probe was slow) are asked for again, a few times, so the
  // one-click offer still shows instead of the generic empty state. A
  // starting host's answer (`pending`) is asked for again too, and once it
  // is ready (`loadNewSpaceHostData`).
  const needsOptions = optionsPending(s.options);
  useEffect(() => {
    if (!offered || !needsOptions) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let live = true;
    void ctx.ready.then(loadHostData).then(() => {
      if (live && tries < FIRST_SPACE_RETRIES) timer = setTimeout(() => setTries((n) => n + 1), FIRST_SPACE_RETRY_MS);
    });
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [offered, ctx.ready, tries, needsOptions]);
  const offers = offered && s.options ? firstSpaceOffers(core, env) : [];
  return { offers, macosVmsRunning: s.options?.macosVmsRunning ?? null };
}

/**
 * The host's requests for New Space (the SwiftUI app's menu items and
 * notch, `spaces.newRequested`): opens the wizard once the core is ready,
 * and presets "Run on" once the host's options (its machines) are in.
 */
export function useNewSpaceRequests(): void {
  const wizard = useNewSpaceWizard();
  const { core, env, s } = useNewSpaceEnv();
  const ready = core.status === "ready";
  useEffect(() => {
    if (!s.requested || !ready || !wizard.offered) return;
    const { on } = s.requested;
    update((x) => ({ ...x, requested: null }));
    wizard.show(on);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.requested, ready, wizard.offered]);
  // The machine is offered once the host's options and machines are in:
  // until then the request waits (the wizard opens on the default).
  const offeredNow = Boolean(
    ready && s.open && s.state && s.placeOn && wizardView(core, s.state, env).placements.some((p) => p.id === s.placeOn && p.enabled),
  );
  useEffect(() => {
    if (!offeredNow || !s.placeOn) return;
    const on = s.placeOn;
    update((x) => ({
      ...x,
      placeOn: null,
      state: x.state ? wizardReduce(core, x.state, { type: "choose-placement", on }, env) : x.state,
    }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [offeredNow]);
}

export interface ConnectCloudHook {
  open: boolean;
  view: CloudConnectView | null;
  show(): void;
  close(): void;
  /** Sends an action; Test and Connect run the host's request the core asks for. */
  send(action: CloudConnectAction): Promise<void>;
}

/** "Connect a cloud": the core's sheet over `clouds.*`. */
export function useConnectCloud(): ConnectCloudHook {
  const ctx = useContext(BridgeContext)!;
  const { core, s } = useNewSpaceEnv();
  const { refresh: refreshSettings } = useSettings();
  const ready = core.status === "ready";
  const input = s.connect.pinnedInput ?? (ready ? connectInput(core, s.clouds) : { providers: [] });
  const view = s.connect.open && s.connect.state && ready ? cloudConnectView(core, input, s.connect.state) : null;
  const close = () => update((x) => ({ ...x, connect: { ...CLOSED_NEW_SPACE.connect } }));
  const reduce = (action: CloudConnectAction) =>
    update((x) => (x.connect.state ? { ...x, connect: { ...x.connect, state: cloudConnectReduce(core, input, x.connect.state, action) } } : x));

  return {
    open: s.connect.open,
    view,
    show() {
      if (!ready) return;
      update((x) => ({ ...x, connect: { open: true, state: cloudConnectInitial(core), pinnedInput: null } }));
      void ctx.ready.then(loadHostData);
    },
    close,
    async send(action) {
      reduce(action);
      if (action.type !== "test" && action.type !== "connect") return;
      const state = read().connect.state;
      if (!state) return;
      const next = cloudConnectView(core, input, state);
      // "No cloud" finishes on the spot, with no request at all.
      if (next.done) return close();
      const request = next.request;
      if (!request || !ctx.adapter) return;
      try {
        if (request.kind === "test") {
          const r = await ctx.adapter.call("clouds.test", { target: request.target });
          reduce({ type: "tested", ok: r.ok, account: r.account ?? "", checks: r.checks.map((c) => ({ name: c.name, ok: c.ok, detail: c.detail ?? "" })) });
        } else {
          const r = await ctx.adapter.call("clouds.connect", { target: request.target, makeDefault: request.make_default });
          reduce({ type: "connected", label: r.label ?? r.title });
          const store = await ctx.ready;
          await Promise.all([loadHostData(store), refreshSettings()]);
          close();
        }
      } catch (e) {
        reduce({ type: "failed", error: isUnsupported(e) ? "This app can't connect a cloud yet." : e instanceof Error ? e.message : String(e) });
      }
    },
  };
}

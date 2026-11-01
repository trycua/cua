// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** New Space and "Connect a cloud" for components (see `new-space.ts`). */

import { useContext, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { isUnsupported } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { CloudConnectWire, CloudSheetAction, CloudSheetView, CloudTarget, RuntimeFix, RuntimeSwitch, WizardAction, WizardEnv, WizardView } from "./contracts/new-space";
import type { Space, SpaceOs } from "./contracts/spaces";
import { useMachines, useSettings, useSpaces } from "./hooks";
import { newPendingId } from "./store";
import {
  CLOSED_NEW_SPACE,
  cloudSheetInitial,
  cloudSheetInput,
  cloudSheetReduce,
  cloudSheetView,
  firstSpaceOffers,
  localBlocked,
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
  const { data: machines } = useMachines();
  const core = ctx.core;
  const env =
    s.pinnedEnv ??
    (core.status === "ready"
      ? wizardEnv(core, {
          options: s.options,
          clouds: s.clouds,
          defaultLocation: settings?.values.defaultLocation,
          machines,
        })
      : ({ defaultLocation: "local", localAvailable: true, maxCpus: 8 } as WizardEnv));
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
  /** Opens "Add a machine by its address" (New Space's menu): the address form, not the wizard's steps. */
  showAddress(): void;
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
export const RUNTIME_SWITCH_ROW: Record<string, string> = { "runtime.lume": "runtime:vm", "runtime.linux": "runtime:container" };

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
    showAddress() {
      if (!offered) return;
      const state = wizardReduce(core, wizardInitial(core, env), { type: "show-address" }, env);
      update((x) => ({ ...x, open: true, pinnedEnv: null, state, placeOn: null }));
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
  /** False while the host's options (its runtimes, storage) are still loading. */
  known: boolean;
  /** Why an image can't be created on this machine now (the offers' rule), or null. */
  blockedReason(ref: string): string | null;
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
  // Without the wizard (no core, or a host without its options) nothing is known to block a create.
  const known = !offered || Boolean(s.options && !needsOptions);
  return {
    offers,
    known,
    blockedReason: (ref) => (offered && s.options ? localBlocked(core, env, ref) : null),
    macosVmsRunning: s.options?.macosVmsRunning ?? null,
  };
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
  view: CloudSheetView | null;
  /** Opens the sheet: the picker, or one cloud's sheet (`show("aws")`, from a cloud's page);
   * with `limits`, the sheet that changes a connected cloud's budget and size. */
  show(cloud?: string, options?: { limits?: boolean }): void;
  close(): void;
  /** Sends an action; Press runs the host's request the core asks for. */
  send(action: CloudSheetAction): Promise<void>;
  /** Hetzner's API token as typed: memory only, dropped when Connect finishes or the sheet closes. */
  token: string;
  setToken(text: string): void;
  /** Opens a page in the person's browser. */
  openPage(url: string): void;
}

/** The core's request target, with the held token attached when it asks (and `uses_token` removed). */
export function withToken(target: CloudTarget, token: string): CloudTarget {
  const out: CloudTarget = { ...target };
  delete out.uses_token;
  if (target.uses_token && token) out.token = token;
  return out;
}

/** The first check that did not pass, as one line ("permissions: ec2:RunInstances was denied"). */
const failedCheck = (r: CloudConnectWire) => {
  const c = r.checks?.find((x) => !x.ok);
  return c ? (c.detail ? `${c.name}: ${c.detail}` : c.name) : "";
};

/** "Connect a cloud": the core's picker and sheets over `clouds.*`. */
export function useConnectCloud(): ConnectCloudHook {
  const ctx = useContext(BridgeContext)!;
  const { core, s } = useNewSpaceEnv();
  const { refresh: refreshSettings } = useSettings();
  const ready = core.status === "ready";
  const input = s.connect.pinnedInput ?? (ready ? cloudSheetInput(core, s.clouds) : { providers: [] });
  const view = s.connect.open && s.connect.state && ready ? cloudSheetView(core, input, s.connect.state) : null;
  // The token never enters the core, the session store or any saved state.
  const [token, setTokenState] = useState("");
  const tokenRef = useRef("");
  const setToken = (text: string) => {
    const was = tokenRef.current !== "";
    tokenRef.current = text;
    setTokenState(text);
    if (was !== (text !== "")) reduce({ type: "set-token-entered", entered: text !== "" });
  };
  const dropToken = () => {
    tokenRef.current = "";
    setTokenState("");
  };
  const close = () => {
    dropToken();
    update((x) => ({ ...x, connect: { ...CLOSED_NEW_SPACE.connect } }));
  };
  const reduce = (action: CloudSheetAction) =>
    update((x) => (x.connect.state ? { ...x, connect: { ...x.connect, state: cloudSheetReduce(core, input, x.connect.state, action) } } : x));
  const openPage = (url: string) => void ctx.adapter?.call("session.openExternal", { url }).catch(() => {});

  return {
    open: s.connect.open,
    view,
    token,
    setToken,
    openPage,
    show(cloud, options) {
      if (!ready) return;
      dropToken();
      const limits = options?.limits === true;
      const first = cloudSheetInitial(core, input, cloud ?? null, limits);
      update((x) => ({ ...x, connect: { open: true, state: first, pinnedInput: x.connect.pinnedInput } }));
      // A cloud's sheet starts from what the link is set to (its region, budget and size). Opened from a page
      // before the clouds were read here, it fills in once they are, unless the person changed something.
      void ctx.ready.then(loadHostData).then(() => {
        const now = read();
        if (!cloud || !now.connect.open || now.connect.pinnedInput || JSON.stringify(now.connect.state) !== JSON.stringify(first)) return;
        const state = cloudSheetInitial(core, cloudSheetInput(core, now.clouds), cloud, limits);
        update((x) => ({ ...x, connect: { ...x.connect, state } }));
      });
    },
    close,
    async send(action) {
      reduce(action);
      if (action.type !== "press") return;
      const state = read().connect.state;
      if (!state) return;
      const request = cloudSheetView(core, input, state).request;
      if (!request || !ctx.adapter) return;
      try {
        const r = await ctx.adapter.call("clouds.connect", { target: withToken(request, tokenRef.current), makeDefault: false });
        const status = r.status ?? (r.connected ? "connected" : "failed");
        if (status === "connected" || status === "waiting-for-approval") dropToken();
        reduce({
          type: "answered",
          status,
          open: r.next?.open ?? "",
          paste: r.next?.paste ?? "",
          why: r.next?.why ?? "",
          failedCheck: failedCheck(r),
        });
        // AWS opens its console where the trust is made: once, when the sheet gets there.
        if (status === "needs-trust" && r.next?.open && !request.check) openPage(r.next.open);
        if (status === "connected" || status === "waiting-for-approval") {
          const store = await ctx.ready;
          await Promise.all([loadHostData(store), refreshSettings(), store.refresh("machines")]);
          close();
        }
      } catch (e) {
        dropToken();
        reduce({ type: "failed", error: isUnsupported(e) ? "This app can't connect a cloud yet." : e instanceof Error ? e.message : String(e) });
      }
    },
  };
}

export interface CloudActionsHook {
  /** Forgets a cloud; with `deleteResources`, deletes what Cua created there first. Rejects with the host's words. */
  disconnect(provider: string, deleteResources: boolean): Promise<void>;
  /** Approves (or denies) what an agent set up. Only where the host can (`can_approve`). */
  approve(provider: string, approve: boolean): Promise<void>;
  /** Opens a page in the person's browser. */
  openPage(url: string): void;
}

/** What a cloud's page does: Disconnect, Approve and Deny, links. The clouds are read again after each. */
export function useCloudActions(): CloudActionsHook {
  const ctx = useContext(BridgeContext)!;
  const again = async () => {
    const store = await ctx.ready;
    await Promise.all([loadHostData(store), store.refresh("machines")]);
  };
  return {
    async disconnect(provider, deleteResources) {
      if (!ctx.adapter) return;
      await ctx.adapter.call("clouds.disconnect", { provider, deleteResources });
      await again();
    },
    async approve(provider, approve) {
      if (!ctx.adapter) return;
      await ctx.adapter.call("clouds.approve", { provider, approve });
      await again();
    },
    openPage: (url) => void ctx.adapter?.call("session.openExternal", { url }).catch(() => {}),
  };
}

/* ---- Windows: the runtime fix ("Install Docker Desktop") ------------------ */

/** How often the options are read again while a fix runs, and for how long after one started. */
export const RUNTIME_FIX_POLL_MS = 3_000;
export const RUNTIME_FIX_FOLLOW_MS = 15 * 60_000;

export interface RuntimeFixHook {
  /** What the host offers beside "no local runtime" (null when it offers nothing, or a runtime is ready). */
  fix: RuntimeFix | null;
  /** Starts the fix's action; the options are read again until it settles. */
  run(): Promise<void>;
  /** Why the host refused to start it. */
  error: string | null;
}

/**
 * The host's fix for a machine with no local runtime (Windows: install or
 * start Docker Desktop). While it runs, or for a while after it started,
 * the options are read again so the reason, the button and the wizard's
 * Continue follow the install and the engine's start.
 */
export function useRuntimeFix(): RuntimeFixHook {
  const ctx = useContext(BridgeContext)!;
  const s = useSyncExternalStore(subscribe, read, read);
  const local = s.options?.local;
  const fix = local && !local.available ? (local.fix ?? null) : null;
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const following = Boolean(fix?.busy) || (startedAt !== null && Date.now() - startedAt < RUNTIME_FIX_FOLLOW_MS && !local?.available);
  useEffect(() => {
    if (!following) return;
    const timer = setInterval(() => void ctx.ready.then(loadHostData), RUNTIME_FIX_POLL_MS);
    return () => clearInterval(timer);
  }, [following, ctx.ready]);
  return {
    fix,
    error,
    async run() {
      const action = fix?.action;
      if (!action || !ctx.adapter) return;
      setError(null);
      try {
        await ctx.adapter.call("spaces.fixRuntime", { action });
        setStartedAt(Date.now());
        await ctx.ready.then(loadHostData);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
  };
}


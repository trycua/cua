// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Connect, as Settings → Connectors (redesign), an app's page, a Space's
 * "isn't connected yet" note and Ask first share it: one place that knows
 * which connects are running, what failed, and which provider sheet is
 * open. The steps and words are the core's (`connectors.*`); the host
 * calls are the ones Settings already makes:
 *
 * - Cua's tools in an app: `agents.connect {signIn: false}` (and
 *   Background computer use, best effort);
 * - the app's own token command (`claude setup-token`, one browser
 *   approval): `agents.connect {signIn: true}`, only where the core's plan
 *   says so. Other apps never get `signIn: true` here, so Cua doesn't read
 *   their own sign-in files;
 * - a pasted key: `agentKeys.set`; Sign out: `agentKeys.remove`.
 */

import { useCallback, useEffect } from "react";
import { create } from "zustand";

import {
  appDetail,
  connectorsInputOf,
  connectorsView,
  connectPlan,
  providerSheet,
  useAgentKeys,
  useBridge,
  type AppDetailView,
  type ConnectorsInput,
  type ConnectorsView,
  type ProviderSheetView,
  type ProviderWay,
} from "@/bridge";
import { connectedAppsState, setConnectedAppsState, useConnectedApps, useConnectedAppsState, type ConnectedApps } from "@/hooks/use-connected-apps";
import { withSwitch } from "@/lib/connected-apps";

type Step = ConnectorsInput["busy"][number]["step"];

interface ConnectState {
  busy: ConnectorsInput["busy"];
  failures: ConnectorsInput["failures"];
  /** The provider sheet open now, and what opened it (an app's Connect continues there). */
  sheet: { provider: string; forApp: string | null } | null;
  /** Provider keys your login shell has set (names), read when a sheet opens. */
  shellKeys: string[];
  /** A device-code sign-in waiting for its code to be entered. */
  device: { app: string; provider: string; url: string; code: string } | null;
}

const useConnectState = create<ConnectState>(() => ({ busy: [], failures: [], sheet: null, shellKeys: [], device: null }));
const set = useConnectState.setState;

const start = (id: string, step: Step) =>
  set((s) => ({ busy: [...s.busy.filter((b) => b.id !== id), { id, step }], failures: s.failures.filter((f) => f.id !== id) }));
const stop = (id: string) => set((s) => ({ busy: s.busy.filter((b) => b.id !== id) }));
const fail = (id: string, line: string) => set((s) => ({ failures: [...s.failures.filter((f) => f.id !== id), { id, line }] }));

/** Whether a provider sheet is open (cheap: the host mounts the sheet only then). */
export const useProviderSheetOpen = () => useConnectState((s) => s.sheet !== null || s.device !== null);

/** The device-code sign-in waiting now (its code and page), for its sheet. */
export const useDeviceSignIn = () => useConnectState((s) => s.device);
export const closeDeviceSignIn = () => set({ device: null });

/** Tests start clean. */
export function resetConnectState(): void {
  set({ busy: [], failures: [], sheet: null, shellKeys: [], device: null });
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** How a connect ended: `connected`, waiting on the provider sheet (`sheet`), or `failed`. */
export type ConnectOutcome = "connected" | "sheet" | "failed";

export interface Connectors {
  apps: ConnectedApps;
  input: ConnectorsInput;
  view: ConnectorsView | null;
  detail(agent: string): AppDetailView | null;
  /** The provider sheet open now. */
  sheet: ProviderSheetView | null;
  /** One Connect for an app: Cua's tools, then its sign-in (its token command, or the provider's sheet). */
  connectApp(agent: string): Promise<ConnectOutcome>;
  openProvider(provider: string, forApp?: string | null): void;
  closeProvider(): void;
  /** Runs one of the sheet's ways (`key` with the pasted value). */
  runWay(way: ProviderWay, value?: string): Promise<boolean>;
  signOut(provider: string): Promise<void>;
  /** An app page's switch: Cua's skills, its MCP server, or keeping it set up. */
  setToggle(agent: string, id: "skills" | "mcp" | "auto", on: boolean): Promise<void>;
  /** Takes Cua out of the app (and turns Keep it set up off). */
  removeFromApp(agent: string): Promise<void>;
  reload(): Promise<void>;
}

export function useConnectors(): Connectors {
  const { core, data } = useBridge();
  const apps = useConnectedApps();
  const keys = useAgentKeys({ watch: true });
  const app = useConnectedAppsState();
  const st = useConnectState();
  const input = connectorsInputOf(apps.rows, keys.input, { auto: app.auto === true, off: app.off, busy: st.busy, failures: st.failures, shellKeys: st.shellKeys });
  const view = connectorsView(core, input);
  const sheet = st.sheet ? providerSheet(core, input, st.sheet.provider) : null;

  const reload = useCallback(async () => {
    await Promise.all([apps.reload(), keys.refresh()]);
  }, [apps, keys]);

  /** An app's device-code sign-in: the code shows in its sheet while the host waits for it to be entered. */
  const runDevice = useCallback(
    async (app: string, provider: string): Promise<boolean> => {
      if (!data) return false;
      const id = `provider:${provider}`;
      start(id, "signing-in");
      start(`app:${app}`, "signing-in");
      try {
        const started = await data.call("agents.signInStart", { agent: app });
        set({ device: { app, provider, url: started.url, code: started.code } });
        const r = await data.call("agents.signInWait", { id: started.id });
        if (!r.imported) {
          fail(id, r.line ?? "The sign-in didn't finish. Try again.");
          return false;
        }
        await reload();
        return true;
      } catch (e) {
        fail(id, message(e));
        return false;
      } finally {
        set({ device: null });
        stop(id);
        stop(`app:${app}`);
      }
    },
    [data, reload],
  );

  const connectApp = useCallback(
    async (agent: string): Promise<ConnectOutcome> => {
      if (!data) return "failed";
      const id = `app:${agent}`;
      const plan = connectPlan(core, input, agent);
      if (!plan) return "failed";
      try {
        if (plan.device) {
          if (plan.tools) {
            start(id, "connecting");
            await data.call("agents.connect", { agents: [agent], signIn: false });
            await data.call("agents.setupDriver", { agents: [agent] }).catch(() => undefined);
            setConnectedAppsState(withSwitch(connectedAppsState(), agent, true));
          }
          return (await runDevice(agent, plan.tokenProvider ?? "openai")) ? "connected" : "failed";
        }
        if (plan.tools || plan.appToken) {
          start(id, plan.appToken ? "signing-in" : "connecting");
          if (plan.tokenProvider) start(`provider:${plan.tokenProvider}`, "signing-in");
          const [result] = await data.call("agents.connect", { agents: [agent], signIn: plan.appToken });
          if (plan.tools) await data.call("agents.setupDriver", { agents: [agent] }).catch(() => undefined);
          setConnectedAppsState(withSwitch(connectedAppsState(), agent, true));
          await reload();
          if (result && plan.tools && !result.tools_ok) {
            fail(id, result.tools_line ?? "Couldn't add Cua to it.");
            return "failed";
          }
          if (plan.appToken && !result?.sign_in?.imported) {
            fail(id, result?.sign_in?.line ?? "The sign-in didn't finish. Try again.");
            return "failed";
          }
        }
        if (plan.provider) {
          set({ sheet: { provider: plan.provider, forApp: agent } });
          return "sheet";
        }
        return "connected";
      } catch (e) {
        fail(id, message(e));
        return "failed";
      } finally {
        stop(id);
        if (plan.tokenProvider) stop(`provider:${plan.tokenProvider}`);
      }
    },
    [core, data, input, reload, runDevice],
  );

  const runWay = useCallback(
    async (way: ProviderWay, value?: string): Promise<boolean> => {
      const open = useConnectState.getState().sheet;
      if (!data || !open || !sheet) return false;
      const id = `provider:${open.provider}`;
      try {
        if (way.id === "app-token" && way.app) {
          set({ sheet: null });
          start(id, "signing-in");
          const [r] = await data.call("agents.connect", { agents: [way.app], signIn: true });
          if (!r?.sign_in?.imported) {
            fail(id, r?.sign_in?.line ?? "The sign-in didn't finish. Try again.");
            return false;
          }
        } else if (way.id === "app-device" && way.app) {
          set({ sheet: null });
          return await runDevice(way.app, open.provider);
        } else if (way.id === "shell" && way.env) {
          start(id, "connecting");
          await data.call("agentKeys.fromShell", { env: way.env });
          await keys.refresh();
          set({ sheet: null });
        } else if (way.id === "key" && value?.trim()) {
          start(id, "connecting");
          await keys.save(sheet.keyProvider, value.trim(), sheet.keyEnv);
          set({ sheet: null });
        } else {
          return false;
        }
        await reload();
        return true;
      } catch (e) {
        fail(id, message(e));
        return false;
      } finally {
        stop(id);
      }
    },
    [data, sheet, keys, reload, runDevice],
  );

  const signOut = useCallback(
    async (provider: string) => {
      const row = view?.providers.find((p) => p.id === provider);
      if (!row) return;
      const id = `provider:${provider}`;
      start(id, "signing-out");
      try {
        for (const env of row.signOutEnvs) await keys.remove(env);
      } catch (e) {
        fail(id, message(e));
      } finally {
        stop(id);
      }
    },
    [view, keys],
  );

  const setToggle = useCallback(
    async (agent: string, which: "skills" | "mcp" | "auto", on: boolean) => {
      if (which === "auto") {
        setConnectedAppsState(withSwitch(connectedAppsState(), agent, on));
        return;
      }
      if (!data) return;
      const id = `app:${agent}`;
      start(id, "connecting");
      try {
        // Only that part: in, or out (the other stays as it is).
        const part = { skills: which === "skills", mcp: which === "mcp" };
        if (on) await data.call("agents.connect", { agents: [agent], signIn: false, ...part });
        else await data.call("agents.disconnect", { agents: [agent], signIn: false, ...part });
        await reload();
      } catch (e) {
        fail(id, message(e));
      } finally {
        stop(id);
      }
    },
    [data, reload],
  );

  const removeFromApp = useCallback(
    async (agent: string) => {
      if (!data) return;
      const id = `app:${agent}`;
      start(id, "connecting");
      try {
        await data.call("agents.disconnect", { agents: [agent], signIn: false });
        setConnectedAppsState(withSwitch(connectedAppsState(), agent, false));
        await reload();
      } catch (e) {
        fail(id, message(e));
      } finally {
        stop(id);
      }
    },
    [data, reload],
  );

  return {
    apps,
    input,
    view,
    detail: (agent) => appDetail(core, input, agent),
    sheet,
    connectApp,
    openProvider: (provider, forApp = null) => {
      set({ sheet: { provider, forApp } });
      // Which keys your shell has (names only), for "Use … from your shell".
      void data
        ?.call("agentKeys.shell", {})
        .then((r) => set({ shellKeys: r.names }))
        .catch(() => set({ shellKeys: [] }));
    },
    closeProvider: () => set({ sheet: null }),
    runWay,
    signOut,
    setToggle,
    removeFromApp,
    reload,
  };
}

/** Re-reads once when a page that connects mounts (the keys may have changed in the CLI). */
export function useConnectorsFresh(c: Connectors): void {
  useEffect(() => {
    void c.reload();
    // Once per mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}

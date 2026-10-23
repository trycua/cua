// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run every host shows (0.10.0): pick any agents (or none), then
 * "Your agent can use this Mac". The words, the rows, when Done is on and the
 * to-do list on Home are the app core's (`firstRun.view`, `firstRun.todos`,
 * `cua-spaces-app-core/src/first_run.rs`); this module gathers what the
 * core reads (the agent setup, `host.status`, the sign-in, the extra
 * Space's create) and runs the buttons.
 *
 * Step 1's ticks, the agents' own sign-ins (`agents.signIn` starts
 * `codex login` on this computer; `agents.setup` reports `signedIn`) and the
 * provider keys (`agentKeys.list`) are all per computer, and the page says so.
 *
 * What is real and what isn't (see `first_run.rs` for the detail):
 * - "Cua is in the agent's settings": `agents.setup`'s `configured`. Real.
 * - "The open session reloaded": no process check exists yet, so the row
 *   only says to restart the agent if it was open, until the agent uses Cua.
 * - "The agent used Cua": a connection in this computer's access log
 *   (`host.status` `recentAccess`) since the setup screen opened. Partly
 *   real: only some calls are logged, and an agent names itself.
 * - This computer: `host.status` and the sign-in. Real.
 *
 * The extra Space's create runs in the host (the store's create), so it
 * keeps going when the window closes; the host also posts a system
 * notification when it is ready (`notifyWhenReady`).
 */

import { useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { AgentSetupRow } from "./contracts/agents";
import type { HostStatus } from "./contracts/host";
import type {
  FirstRunAgent,
  FirstRunExtra,
  FirstRunInput,
  FirstRunOffer,
  FirstRunPlan,
  FirstRunStep,
  FirstRunTodos,
  FirstRunView,
} from "./contracts/first-run";
import { useSession, useSettings, useSpaces } from "./hooks";
import { hostOs } from "./host-os";
import { useFirstSpaceOffers } from "./new-space-hooks";
import { onboardingInitial, onboardingTelemetry, onboardingView, readSavedOnboarding, writeSavedOnboarding } from "./onboarding";
import type { OnboardingFlowState } from "./contracts/onboarding";
import { useRelayUsage } from "./relay-usage";
import { useAgentKeys } from "./agent-keys";
import { newPendingId } from "./store";
import { extraName, spokenName } from "./space-names";
import { runCreateFix } from "./create-retry";
import type { CreateFix } from "./contracts/spaces";

export { extraName, spokenName };

/** The troubleshooting page (`creating::TROUBLESHOOT_URL`). */
const TROUBLESHOOT_URL = "https://cua.ai/docs/spaces/guides/troubleshoot#find-your-reason";

/** The card's input without the page-only `fix` (the core's first-run
 * input has no such field). */
function withoutFix(extra: FirstRunExtra & { fix?: CreateFix | null }): FirstRunExtra {
  const { fix: _fix, ...rest } = extra;
  return rest;
}

/** The extra Space failed the same way twice: the card shows the core's fix
 * for that cause (its sentence, and its button as `fix`) instead of a plain
 * Try again, as the Space's own page does. */
function withExtraFix(view: FirstRunView, fix: CreateFix | null): FirstRunView {
  const card = view.setup?.extra;
  if (!fix || !view.setup || card?.phase !== "failed") return view;
  return { ...view, setup: { ...view.setup, extra: { ...card, note: `${card.note} ${fix.text}`, action: { id: "fix", label: fix.label } } } };
}
import { spaceState } from "../lib/spaces";



/* ---- The core ------------------------------------------------------------------- */

export function firstRunView(core: CoreClient, input: FirstRunInput): FirstRunView {
  return core.tryCall<FirstRunView>("firstRun.view", { input }) ?? fallbackView(input);
}

export function firstRunTodos(core: CoreClient, input: FirstRunInput): FirstRunTodos | null {
  return core.tryCall<FirstRunTodos>("firstRun.todos", { input }) ?? null;
}

/** When the agent used Cua since `sinceMs` (ms), from the access log. */
export function agentUsedMs(core: CoreClient, status: HostStatus | null, sinceMs: number, agentName: string): number | null {
  const lines = status?.recentAccess ?? [];
  if (!lines.length) return null;
  const fromCore = core.tryCall<number | null>("firstRun.agentUsed", { lines, sinceMs, agentName });
  if (fromCore !== undefined) return fromCore;
  const hits = lines.filter((l) => l.atMs >= sinceMs && !/\(background\)$|\(thumbnail|^refused /.test(l.what) && l.who !== "refused");
  return hits.length ? Math.max(...hits.map((l) => l.atMs)) : null;
}

/** "18:12", in the user's own clock. */
export function clockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

/** Without the core: the two screens in their plainest state. Not the
 * product's behaviour; the demo and tests without wasm use it. */
function fallbackView(input: FirstRunInput): FirstRunView {
  const name = input.agents?.find((a) => a.agent === input.agent)?.name ?? "Claude Code";
  const place = input.hostOs === "macos" ? "this Mac" : input.hostOs === "windows" ? "this PC" : "this computer";
  const art = { agent: input.agent ?? "claude-code", computer: "on" as const, extra: "outline" as const };
  if (input.step === "connect") {
    const picked = input.picked ?? (input.agents ?? []).filter((a) => a.installed).map((a) => a.agent);
    const agents = (input.agents ?? []).map((a) => ({ agent: a.agent, name: a.name, checked: picked.includes(a.agent), installed: a.installed, locked: a.configured, note: null }));
    return {
      step: "connect",
      later: "Skip for now",
      connect: {
        title: "A shared Space for every agent.",
        accent: "agent",
        lede: `Connect your agents and give them computers to work on: macOS, Linux or Windows, right here on ${place}.`,
        intro: "Pick any, or none.",
        agentsLabel: `On ${place}`,
        agents,
        moreLabel: "More agents",
        more: [],
        agentsNote: `Cua adds its skills and MCP server to each one you pick, on ${place} only. Your other computers set up their own.`,
        credentialsLabel: "Sign-ins and keys",
        credentialsNote: `They stay on ${place}. Other computers keep their own.`,
        credentials: [],
        primary: { id: "continue", label: "Continue" },
        connects: picked,
        busy: input.connecting ? `Adding Cua to ${name}…` : null,
        error: input.connectError,
      },
      setup: null,
      art: { ...art, extra: "none" },
      dots: 2,
      current: 0,
    };
  }
  const ok = input.signedIn && Boolean(input.host?.configured);
  return {
    step: "setup",
    later: "Skip for now",
    connect: null,
    setup: {
      title: ok ? `Your agent can use ${place}.` : "Checking your setup.",
      rows: [
        { id: "agent", state: "ok", title: `${name} can use Cua`, note: `We added Cua to ${name}'s settings.`, action: null, settingsUrl: null },
        ok
          ? { id: "computer", state: "ok", title: `${place[0]!.toUpperCase()}${place.slice(1)} is your first Space`, note: "Ready now. Your agent can see the screen, click and type.", action: null, settingsUrl: null }
          : { id: "computer", state: "fix", title: "Sign in to Cua", note: `So your agent can find ${place} and your other computers.`, action: { id: "sign-in", label: "Sign in" }, settingsUrl: null },
      ],
      prompt: { heading: `Try this in ${name}`, text: "List my computers and tell me what's on this Mac", note: `This works now, on ${place}.`, copy: "Copy" },
      extra: { phase: "not-now", title: "No extra Space for now", tag: null, note: "Add one from Home whenever you like.", choices: [], action: { id: "add", label: "Add a Space" }, fraction: null, footnote: null },
      upsell: null,
      primary: ok ? { id: "done", label: "Done", enabled: true, note: null } : { id: "finish-later", label: "Finish later", enabled: true, note: "We'll keep a to-do list on Home." },
    },
    art,
    dots: 2,
    current: 1,
  };
}

/* ---- What the page keeps between launches ----------------------------------------- */

/** The first run's progress (resumes after a relaunch, and feeds Home's
 * to-do list after Finish later). */
export interface FirstRunSaved {
  step: FirstRunStep;
  /** The agent the first request names: the first one connected (setup id). */
  agent: string | null;
  /** Step 1: the ticks (null: not changed). Step 2: the agents connected. */
  picked: string[] | null;
  /** When the setup screen first showed (the access log counts from here). */
  setupStartedMs: number | null;
  /** When the agent was first seen using Cua. */
  usedAtMs: number | null;
  /** The extra Space's create (its pending row id). */
  extraPendingId: string | null;
  extraOs: string | null;
  /** "Not now" on the extra Space card. */
  extraNotNow: boolean;
  /** Finish later: Home shows the to-do list until it is done or closed. */
  finishedLater: boolean;
  /** The to-do list was closed. */
  todosClosed: boolean;
}

const KEY = "cua-spaces:first-run";
const EMPTY: FirstRunSaved = {
  step: "connect",
  agent: null,
  picked: null,
  setupStartedMs: null,
  usedAtMs: null,
  extraPendingId: null,
  extraOs: null,
  extraNotNow: false,
  finishedLater: false,
  todosClosed: false,
};
/** "Not now" on the plan card: never again. */
export const UPSELL_DISMISSED_KEY = "cua.upsell.dismissed.onboarding";
/** The user entered teammates' emails (a share with other people). */
export const TEAMMATES_KEY = "cua.firstRun.teammates";

let saved: FirstRunSaved | undefined;
const listeners = new Set<() => void>();

function storage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

export function readFirstRun(): FirstRunSaved {
  if (saved) return saved;
  try {
    const raw = storage()?.getItem(KEY);
    saved = raw ? { ...EMPTY, ...(JSON.parse(raw) as Partial<FirstRunSaved>) } : EMPTY;
  } catch {
    saved = EMPTY;
  }
  return saved;
}

export function writeFirstRun(next: Partial<FirstRunSaved> | null): void {
  saved = next ? { ...readFirstRun(), ...next } : EMPTY;
  try {
    if (next) storage()?.setItem(KEY, JSON.stringify(saved));
    else storage()?.removeItem(KEY);
  } catch {
    // Not kept: the flow still works, it just won't resume.
  }
  for (const l of [...listeners]) l();
}

function flag(key: string): boolean {
  try {
    return storage()?.getItem(key) === "1";
  } catch {
    return false;
  }
}

function setFlag(key: string): void {
  try {
    storage()?.setItem(key, "1");
  } catch {
    // Not kept.
  }
  for (const l of [...listeners]) l();
}

/** The share sheet calls this when the user shares with other people. */
export function noteTeammates(): void {
  setFlag(TEAMMATES_KEY);
}

/** Tests: forget the cached copy. */
export function resetFirstRunCache(): void {
  saved = undefined;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => void listeners.delete(l);
};

export function useFirstRunSaved(): FirstRunSaved {
  return useSyncExternalStore(subscribe, readFirstRun, readFirstRun);
}

/** Usage events never get in the way of the flow. */
function track(send: () => void): void {
  try {
    send();
  } catch {
    // An older core that reads the state differently: no event.
  }
}

/* ---- The hook ---------------------------------------------------------------------- */

/** How often the setup screen reads the agent setup and this computer. */
export const FIRST_RUN_POLL_MS = 2_500;

/** The usage-data note and switch on the first page: the words are the app core's
 * (the old Welcome's), on by default, off with one tap. */
export interface FirstRunUsage {
  notice: string;
  linkLabel: string;
  linkUrl: string;
  label: string;
  on: boolean;
  enabled: boolean;
  help: string | null;
}

export interface FirstRunHook {
  view: FirstRunView;
  /** The usage-data note and switch (first page only; none until the host says what the setting is). */
  usage: FirstRunUsage | null;
  setUsage(on: boolean): Promise<void>;
  /** "What is collected". */
  openUsageLink(): void;
  input: FirstRunInput;
  saved: FirstRunSaved;
  /** Every agent the setup knows. */
  agents: AgentSetupRow[];
  /** Step 1: tick or untick an agent. */
  toggle(agent: string, on: boolean): void;
  /** Step 1's Continue: adds Cua to the ticked agents not set up yet (none: nothing to add), then step 2. */
  connect(): Promise<void>;
  /** Starts an agent's own sign-in on this computer (`codex login`). */
  signIn(agent: string): Promise<void>;
  /** A row's or a to-do's button (`sign-in`, `set-up`, `open-settings`, `restart-check`, `connect`). */
  act(id: string, settingsUrl?: string | null): Promise<void>;
  copyPrompt(): Promise<void>;
  /** The extra Space card: `macos`/`linux` start it; `cancel`, `not-now`, `add`, `retry`. */
  extra(id: string): Promise<void>;
  upsell(action: "shown" | "click" | "dismiss"): void;
  /** Done or Finish later. */
  finish(): Promise<void>;
  /** "Set up later": leave for now (the sidebar offers it again). */
  later(): void;
  back(): void;
}

/** Live readings while `active` (the first run, or Home's to-do list). */
export function useFirstRun(active = true, pollMs = FIRST_RUN_POLL_MS): FirstRunHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const { core, adapter, store } = ctx;
  const s = useFirstRunSaved();
  const session = useSession();
  const settings = useSettings();
  const spaces = useSpaces();
  const offersHook = useFirstSpaceOffers();
  const usage = useRelayUsage();
  const flags = useSyncExternalStore(subscribe, () => `${flag(UPSELL_DISMISSED_KEY)}:${flag(TEAMMATES_KEY)}`, () => "false:false");
  const [agents, setAgents] = useState<AgentSetupRow[] | null>(null);
  const [host, setHost] = useState<HostStatus | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [settingUp, setSettingUp] = useState(false);
  const [setupError, setSetupError] = useState<string | null>(null);
  const [signingAgent, setSigningAgent] = useState<string | null>(null);
  const [signInError, setSignInError] = useState<string | null>(null);
  const signingRef = useRef(signingAgent);
  signingRef.current = signingAgent;
  // The provider keys saved here (none until read, or where the app can't read them).
  const agentKeys = useAgentKeys();
  const keysInput = agentKeys.unsupported || agentKeys.isLoading ? null : agentKeys.input;
  const [tick, setTick] = useState(0);
  const shownRef = useRef(false);
  const agentsRef = useRef(agents);
  agentsRef.current = agents;

  const signedIn = Boolean(session.data?.signedIn);
  const signingIn = session.data?.signIn.kind === "starting" || session.data?.signIn.kind === "waiting";

  // The agent setup and this computer, now and every few seconds.
  useEffect(() => {
    if (!active || !adapter) return;
    let live = true;
    let n = 0;
    const read = (forceAgents = false) => {
      // The agent setup runs a command on the host: once connected, it's read now and then
      // (every time while an agent's own sign-in waits for the browser).
      const saved = readFirstRun();
      const want = saved.picked ?? (saved.agent ? [saved.agent] : []);
      const connected = saved.step === "setup" && Boolean(agentsRef.current) && want.every((id) => agentsRef.current!.some((a) => a.agent === id && a.configured));
      if (forceAgents || signingRef.current || !connected || n++ % 6 === 0) {
        void adapter
          .call("agents.setup", {})
          .then((rows) => live && setAgents(rows))
          .catch(() => live && setAgents((a) => a ?? []));
      }
      if (s.step === "setup" || s.finishedLater) {
        void adapter
          .call("host.status", {})
          .then((h) => live && setHost(h))
          .catch(() => {});
      }
    };
    read();
    const focus = () => read(true);
    window.addEventListener("focus", focus);
    const timer = setInterval(() => read(), pollMs);
    return () => {
      live = false;
      clearInterval(timer);
      window.removeEventListener("focus", focus);
    };
  }, [active, adapter, s.step, s.finishedLater, tick, pollMs]);

  const rows: FirstRunAgent[] | null = agents
    ? agents.map((a) => ({ agent: a.agent, name: a.name, installed: a.installed, configured: a.configured, signIn: a.signIn ?? null, signedIn: Boolean(a.signedIn) }))
    : null;
  const lead = core.tryCall<string>("firstRun.leadAgent", { agents: rows ?? [] }) ?? rows?.find((a) => a.installed)?.agent ?? "claude-code";
  // Step 2: the agents connected (an older saved run connected one, `agent`).
  const connectedIds = s.step === "setup" ? (s.picked ?? (s.agent ? [s.agent] : [lead])) : [];
  const agentId = s.step === "setup" ? (connectedIds[0] ?? null) : (s.agent ?? lead);
  const agentNames = connectedIds.map((id) => rows?.find((a) => a.agent === id)?.name).filter((n): n is string => Boolean(n));

  // The first time a connected agent uses Cua since the setup screen opened.
  const usedMs =
    s.usedAtMs ??
    (s.setupStartedMs && agentNames.length
      ? agentNames.map((n) => agentUsedMs(core, host, s.setupStartedMs!, n)).reduce<number | null>((a, b) => (b !== null && (a === null || b > a) ? b : a), null)
      : null);

  // An agent's own sign-in finished (its file is there): stop waiting.
  useEffect(() => {
    if (signingAgent && agents?.some((a) => a.agent === signingAgent && a.signedIn)) setSigningAgent(null);
  }, [signingAgent, agents]);
  useEffect(() => {
    if (usedMs && !s.usedAtMs) writeFirstRun({ usedAtMs: usedMs });
  }, [usedMs, s.usedAtMs]);

  // The extra Space, as the Spaces list shows its create.
  const extra = useMemo<FirstRunExtra & { fix?: CreateFix | null }>(() => {
    if (s.extraPendingId) {
      const id = store?.createdId(s.extraPendingId) ?? null;
      const row = spaces.data?.find((x) => x.id === (id ?? s.extraPendingId));
      if (row) {
        const state = spaceState(row);
        if (state === "creating") {
          return {
            phase: "creating",
            name: spokenName(row.name),
            os: row.os,
            detail: row.progress?.transfer ?? row.progress?.label ?? null,
            fraction: row.progress ? row.progress.permille / 1000 : null,
          };
        }
        if (state === "failed") return { phase: "failed", name: spokenName(row.name), os: row.os, error: row.progress?.error ?? null, fix: row.progress?.fix ?? null };
        return { phase: "ready", name: spokenName(row.name), os: row.os };
      }
      // Asked for, not listed yet.
      return { phase: "creating", os: s.extraOs, name: null, detail: null, fraction: null };
    }
    return { phase: s.extraNotNow ? "not-now" : "none" };
  }, [s.extraPendingId, s.extraOs, s.extraNotNow, spaces.data, store]);

  const offers: FirstRunOffer[] = offersHook.offers.map((o) => ({ os: o.os, time: o.time, size: o.size, blocked: o.blocked }));
  const [dismissed, teammates] = flags.split(":").map((f) => f === "true");
  const plan: FirstRunPlan | null = usage
    ? {
        metered: usage.metered,
        plan: usage.plan,
        sessionsLimit: usage.liveSessionsLimit,
        hoursLimit: usage.hoursLimit,
        teammates: Boolean(teammates),
        dismissed: Boolean(dismissed),
        accountApp: settings.data?.accountUrl ?? "https://app.cua.ai",
      }
    : null;

  const input: FirstRunInput = {
    step: s.step,
    hostOs: hostOs(),
    agents: rows,
    agent: agentId,
    picked: s.picked,
    keys: keysInput,
    agentSigningIn: signingAgent,
    agentSignInError: signInError,
    connecting,
    connectError,
    // No process check yet (see the module comment).
    agentOpenBeforeSetup: null,
    agentUsedAt: usedMs ? clockTime(usedMs) : null,
    signedIn,
    signingIn,
    host: host
      ? {
          configured: host.configured,
          running: Boolean(host.service?.running),
          online: host.online ?? null,
          permissions: host.permissions.map((p) => ({ id: p.id, granted: Boolean(p.granted), settingsUrl: p.settingsUrl ?? null })),
          error: setupError ?? host.error ?? null,
        }
      : null,
    hostSettingUp: settingUp,
    offers,
    extra,
    plan,
  };
  const view = withExtraFix(firstRunView(core, { ...input, extra: withoutFix(extra) }), extra.fix ?? null);

  const welcome = settings.data?.telemetry
    ? onboardingView(core, { ...onboardingInitial(core, null), step: "welcome", telemetry: onboardingTelemetry(settings.data.telemetry) })
    : null;
  const usageNote: FirstRunUsage | null =
    s.step === "connect" && welcome?.usage && welcome.notice && welcome.noticeLinkLabel && welcome.noticeLinkUrl
      ? {
          notice: welcome.notice,
          linkLabel: welcome.noticeLinkLabel,
          linkUrl: welcome.noticeLinkUrl,
          label: welcome.usage.label,
          on: welcome.usage.on,
          enabled: welcome.usage.enabled,
          help: welcome.usage.help ?? null,
        }
      : null;

  // The run starts (once a page shows): the usage event the old Welcome sent.
  useEffect(() => {
    const t = settings.data?.telemetry;
    if (!active || shownRef.current || !store || !t) return;
    shownRef.current = true;
    track(() => store.trackOnboarding({ ...onboardingInitial(core, null), telemetry: onboardingTelemetry(t) }, { type: "start" }));
  }, [active, store, settings.data?.telemetry]);

  // The funnel's events, in the old flow's shape: started on step 1, left on step 2.
  const telemetryState = (): OnboardingFlowState => ({
    ...onboardingInitial(core, session.data?.identity ?? null),
    step: s.step === "connect" ? "welcome" : "agents",
    runCounted: true,
    telemetry: settings.data?.telemetry ? onboardingTelemetry(settings.data.telemetry) : null,
  });

  const refresh = () => setTick((n) => n + 1);

  const setUpThisComputer = async () => {
    if (!store || settingUp) return;
    setSettingUp(true);
    setSetupError(null);
    try {
      const ok = await store.thisMachine.quickSetUp(session.data?.identity ?? null);
      if (!ok) setSetupError(store.thisMachine.get().setupError?.message ?? "Couldn't set it up.");
    } finally {
      setSettingUp(false);
      refresh();
    }
  };

  // Signed in on a computer that isn't set up yet: set it up, once.
  const autoSetUp = useRef(false);
  useEffect(() => {
    if (!active || s.step !== "setup" || !signedIn || !host || host.configured || autoSetUp.current) return;
    autoSetUp.current = true;
    void setUpThisComputer();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, s.step, signedIn, host?.configured]);

  // Step 1's ticks as the core decided them (the installed ones until the user changes one).
  const ticked = (): string[] => {
    const c = view.connect;
    return c ? [...c.agents, ...c.more].filter((p) => p.checked).map((p) => p.agent) : (s.picked ?? []);
  };

  const toggle = (agent: string, on: boolean) => {
    const now = ticked().filter((id) => id !== agent);
    writeFirstRun({ picked: on ? [...now, agent] : now });
  };

  const connect = async () => {
    if (!adapter || connecting) return;
    // Step 1: the ticked ones; step 2's Connect: the connected ones still without Cua.
    const picked = s.step === "connect" ? ticked() : connectedIds;
    const ids = s.step === "connect" ? (view.connect?.connects ?? picked) : picked.filter((id) => !agents?.some((a) => a.agent === id && a.configured));
    setConnecting(true);
    setConnectError(null);
    try {
      if (ids.length) setAgents(await adapter.call("agents.configure", { agents: ids }));
      writeFirstRun({ step: "setup", picked, agent: picked[0] ?? null, setupStartedMs: readFirstRun().setupStartedMs ?? Date.now() });
    } catch (e) {
      setConnectError(e instanceof Error ? e.message : String(e));
    } finally {
      setConnecting(false);
    }
  };

  const signIn = async (agent: string) => {
    if (!adapter) return;
    setSignInError(null);
    setSigningAgent(agent);
    try {
      setAgents(await adapter.call("agents.signIn", { agent }));
    } catch (e) {
      setSigningAgent(null);
      setSignInError(e instanceof Error ? e.message : String(e));
    }
  };

  const copyPrompt = async () => {
    const text = view.setup?.prompt?.text ?? firstRunView(core, { ...input, step: "setup" }).setup?.prompt?.text ?? "";
    await navigator.clipboard?.writeText(text).catch(() => {});
  };

  const extraAct = async (id: string) => {
      if (id === "macos" || id === "linux") {
        const offer = offersHook.offers.find((o) => o.os === id);
        if (!offer || offer.blocked) return;
        const pendingId = newPendingId();
        writeFirstRun({ extraPendingId: pendingId, extraOs: id, extraNotNow: false });
        spaces.createSpace({ ...offer.args, name: extraName(id, spaces.data ?? []), os: offer.os, notifyWhenReady: true }, pendingId).catch(() => {
          // The row shows the failure; the card reads it from there.
        });
      } else if (id === "cancel" && s.extraPendingId) {
        await spaces.cancelCreate(s.extraPendingId).catch(() => {});
        writeFirstRun({ extraPendingId: null, extraOs: null });
      } else if (id === "not-now") {
        writeFirstRun({ extraNotNow: true });
      } else if (id === "add") {
        writeFirstRun({ extraNotNow: false, extraPendingId: null, extraOs: null });
      } else if (id === "retry" && s.extraPendingId) {
        const next = spaces.retryCreate(s.extraPendingId);
        writeFirstRun({ extraPendingId: next ?? null });
      } else if (id === "fix" && extra.fix) {
        // It failed the same way twice: that cause's fix (the core's).
        await runCreateFix(extra.fix, {
          copy: (text) => navigator.clipboard?.writeText(text),
          openExternal: (url) => session.openExternal(url),
          // No review sheet here: the help page says what each reason means.
          details: () => void session.openExternal(TROUBLESHOOT_URL).catch(() => {}),
        }).catch(() => {});
      }
  };

  return {
    view,
    usage: usageNote,
    setUsage: (on) => settings.updateSetting("telemetry", on),
    openUsageLink: () => {
      if (usageNote) void session.openExternal(usageNote.linkUrl).catch(() => {});
    },
    input,
    saved: s,
    agents: agents ?? [],
    toggle,
    connect,
    signIn,
    copyPrompt,
    extra: extraAct,
    async act(id, settingsUrl) {
      switch (id) {
        case "sign-in":
          await session.signIn().catch(() => {});
          break;
        case "set-up":
          autoSetUp.current = true;
          await setUpThisComputer();
          break;
        case "open-settings":
          if (settingsUrl && adapter) {
            await (settingsUrl.startsWith("x-apple.systempreferences:")
              ? adapter.call("host.openSettings", { url: settingsUrl })
              : session.openExternal(settingsUrl)
            ).catch(() => {});
          }
          break;
        case "connect":
          await connect();
          break;
        case "copy":
          await copyPrompt();
          break;
        case "add":
          await extraAct("add");
          break;
      }
      refresh();
    },
    upsell(action) {
      const card = view.setup?.upsell;
      if (!card) return;
      try {
        const signals = core.tryCall<unknown[]>("telemetry.upsell", { surface: "onboarding", action });
        if (signals?.length && store) void store.telemetry.track(signals);
      } catch {
        // An older core without the surface: no event.
      }
      if (action === "click") void session.openExternal(card.url).catch(() => {});
      else if (action === "dismiss") setFlag(UPSELL_DISMISSED_KEY);
    },
    async finish() {
      const later = view.setup?.primary.id === "finish-later";
      if (store) track(() => store.trackOnboardingFinished(telemetryState()));
      await session.completeOnboarding(host?.configured ? "host" : "client");
      writeFirstRun({ finishedLater: later, todosClosed: false });
    },
    later() {
      if (store) track(() => store.trackOnboardingSkipped(telemetryState()));
      store?.cancelSignIn();
      // The shell stops opening it (the sidebar offers it again).
      writeSavedOnboarding({ ...readSavedOnboarding(), skipped: true });
      writeFirstRun({ finishedLater: true });
    },
    back() {
      writeFirstRun({ step: "connect" });
    },
  };
}

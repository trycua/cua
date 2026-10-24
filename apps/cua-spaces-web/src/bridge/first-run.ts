// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run every host shows: Welcome, Pick your agent, Connect your
 * other devices (an optional sign-in), then Try it in your agent. The words,
 * the agents' order, which button does what and the to-do list on Home are
 * the app core's (`firstRun.view`, `firstRun.todos`,
 * `cua-spaces-app-core/src/first_run.rs`); this module gathers what the core
 * reads (the agent setup, `host.status`, the sign-in, the first Space's
 * create) and runs the buttons.
 *
 * Nothing starts or connects by itself: each agent is connected by its own
 * Connect (`agents.configure` for that one agent), the sign-in only on "Sign
 * in to connect", and the first Space only on a click. The agents' own
 * sign-ins (`agents.signIn` starts `codex login` on this computer) and the
 * provider keys (`agentKeys.list`) stay on this computer, and the page says so.
 *
 * What is real and what isn't (see `first_run.rs` for the detail):
 * - "Cua is in the agent's settings": `agents.setup`'s `configured`. Real.
 * - "The agent used Cua": a connection in this computer's access log
 *   (`host.status` `recentAccess`) since the try screen opened. Partly
 *   real: only some calls are logged, and an agent names itself.
 * - This computer in the account: `host.status` and the sign-in. Real.
 *
 * The first Space's create runs in the host (the store's create), so it
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
  if (!fix || !view.setup || !card || card.phase !== "failed") return view;
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

/** The order the core leads with (`first_run::PREFERENCE`). */
const PREFERENCE = ["hermes", "claude-code", "codex", "cursor", "openclaw"];
const rank = (id: string) => {
  const i = PREFERENCE.indexOf(id);
  return i < 0 ? PREFERENCE.length : i;
};

/** Without the core: the four screens in their plainest state. Not the
 * product's behaviour; the demo and tests without wasm use it. */
function fallbackView(input: FirstRunInput): FirstRunView {
  const place = input.hostOs === "macos" ? "this Mac" : input.hostOs === "windows" ? "this PC" : "this computer";
  const Place = `${place[0]!.toUpperCase()}${place.slice(1)}`;
  const found = (input.agents ?? []).filter((a) => a.installed || a.configured).sort((a, b) => rank(a.agent) - rank(b.agent));
  const connected = found.filter((a) => a.configured || (input.picked ?? []).includes(a.agent));
  const lead = connected[0] ?? null;
  const steps = ["welcome", "connect", "devices", "setup"] as const;
  const base: FirstRunView = {
    step: input.step,
    later: "Skip for now",
    welcome: null,
    connect: null,
    devices: null,
    setup: null,
    art: { agent: lead?.agent ?? found[0]?.agent ?? "other", computer: "on", extra: input.step === "welcome" ? "none" : "outline" },
    dots: 4,
    current: steps.indexOf(input.step),
  };
  switch (input.step) {
    case "welcome":
      return { ...base, welcome: { title: "Your agents get their own computer.", lede: `Spaces on ${place}, away from your files and your mouse.`, primary: { id: "start", label: "Get started" } } };
    case "connect":
      return {
        ...base,
        connect: {
          title: "Pick your agent.",
          lede: `Found on ${place}. You can add more later.`,
          agents: found.map((a) => {
            const on = connected.includes(a);
            return { agent: a.agent, name: a.name, file: a.mcpConfig ?? null, connected: on, action: on ? null : { id: "connect", label: "Connect" }, note: on ? "Connected" : null };
          }),
          alternatives: [],
          credentials: [],
          credentialsNote: `They stay on ${place}.`,
          primary: { id: "continue", label: "Continue" },
          secondary: connected.length ? null : { id: "skip", label: "Skip" },
          error: input.connectError,
        },
      };
    case "devices":
      return {
        ...base,
        devices: {
          title: input.signedIn ? `${Place} is in your account.` : "Connect your other devices.",
          lede: `Sign in once, and your other computers reach ${place}'s Spaces.`,
          rows: [
            { title: "Your other computers", note: `They join ${place}'s pool` },
            { title: "Your cloud", note: "Hetzner, AWS, Google Cloud or Modal" },
          ],
          status: null,
          primary: input.signedIn ? { id: "continue", label: "Continue" } : { id: "sign-in", label: "Sign in to connect" },
          secondary: input.signedIn ? null : { id: "not-now", label: "Not now" },
          note: input.signedIn ? null : `${Place} works without it.`,
        },
      };
    case "setup":
      return {
        ...base,
        setup: lead
          ? {
              title: `Try it in ${lead.name}.`,
              lede: `Paste this. ${lead.name} makes the Space itself.`,
              prompt: { heading: `Paste this in ${lead.name}`, text: `Make a Linux Space on ${place} and stream the desktop in a new window.`, note: `It runs on ${place}.`, copy: "Copy" },
              status: { id: "agent", state: input.agentUsedAt ? "ok" : "checking", title: input.agentUsedAt ? `${lead.name} used Cua at ${input.agentUsedAt}.` : `Waiting for ${lead.name}…`, note: "", action: null, settingsUrl: null },
              extra: null,
              primary: { id: "done", label: "Done", enabled: true, note: null },
            }
          : {
              title: "Try a first Space.",
              lede: `Spaces run on ${place}, away from your files and your mouse.`,
              prompt: null,
              status: null,
              extra: { phase: "not-now", title: "No Space for now", tag: null, note: "Add one from Home whenever you like.", choices: [], action: { id: "add", label: "Add a Space" }, fraction: null, footnote: null },
              primary: { id: "done", label: "Done", enabled: true, note: null },
            },
      };
  }
}

/* ---- What the page keeps between launches ----------------------------------------- */

/** The first run's progress (resumes after a relaunch, and feeds Home's
 * to-do list after Finish later). */
export interface FirstRunSaved {
  step: FirstRunStep;
  /** An older saved run's one connected agent (setup id). */
  agent: string | null;
  /** The agents connected on Pick your agent. */
  picked: string[] | null;
  /** When the try screen first showed (the access log counts from here). */
  setupStartedMs: number | null;
  /** When the agent was first seen using Cua. */
  usedAtMs: number | null;
  /** The first Space's create (its pending row id). */
  extraPendingId: string | null;
  extraOs: string | null;
  /** "Not now" on the first Space card. */
  extraNotNow: boolean;
  /** Skip for now: the sidebar offers the first run again. */
  finishedLater: boolean;
  /** The to-do list was closed. */
  todosClosed: boolean;
}

const KEY = "cua-spaces:first-run";
const EMPTY: FirstRunSaved = {
  step: "welcome",
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
/** "Not now" on the plan card: never again (no plan shows in the first run now). */
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

/** How often the first run reads the agent setup and this computer. */
export const FIRST_RUN_POLL_MS = 2_500;

/** The usage-data note and switch on Pick your agent: the words are the app
 * core's (the old Welcome's), on by default, off with one tap. */
export interface FirstRunUsage {
  notice: string;
  linkLabel: string;
  linkUrl: string;
  label: string;
  on: boolean;
  enabled: boolean;
  help: string | null;
}

const STEPS: FirstRunStep[] = ["welcome", "connect", "devices", "setup"];

export interface FirstRunHook {
  view: FirstRunView;
  /** The usage-data note and switch (Pick your agent only; none until the host says what the setting is). */
  usage: FirstRunUsage | null;
  setUsage(on: boolean): Promise<void>;
  /** "What is collected". */
  openUsageLink(): void;
  input: FirstRunInput;
  saved: FirstRunSaved;
  /** Every agent the setup knows. */
  agents: AgentSetupRow[];
  /** Pick your agent: adds Cua to this one agent's settings on this computer. */
  connect(agent: string): Promise<void>;
  /** Starts an agent's own sign-in on this computer (`codex login`). */
  signIn(agent: string): Promise<void>;
  /** A screen's button: `start`, `continue`, `skip`, `not-now` (next screen), `sign-in`, `set-up`, `open-settings`, `copy`, `add`. */
  act(id: string, settingsUrl?: string | null): Promise<void>;
  /** Copies `text` (the tab shown), else the selected request. */
  copyPrompt(text?: string): Promise<void>;
  /** Opens a page in the browser (installing an agent). */
  openUrl(url: string): Promise<void>;
  /** The first Space card: `macos`/`linux` start it; `cancel`, `not-now`, `add`, `retry`, `fix`. */
  extra(id: string): Promise<void>;
  /** Done. */
  finish(): Promise<void>;
  /** "Skip for now": leave for now (the sidebar offers it again). */
  later(): void;
  /** The screen before. */
  back(): void;
}

/** Live readings while `active` (the first run, or what reads its state). */
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
  const [connecting, setConnecting] = useState<string | null>(null);
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

  const signedIn = Boolean(session.data?.signedIn);
  const signingIn = session.data?.signIn.kind === "starting" || session.data?.signIn.kind === "waiting";
  // An older saved run may say any step; an unknown one starts over.
  const step: FirstRunStep = STEPS.includes(s.step) ? s.step : "welcome";

  // The agent setup and this computer, now and every few seconds.
  useEffect(() => {
    if (!active || !adapter) return;
    let live = true;
    let n = 0;
    const read = (forceAgents = false) => {
      // The agent setup runs a command on the host: read every time on Pick
      // your agent (and while an agent's own sign-in waits), now and then after.
      if (forceAgents || signingRef.current || step === "connect" || n++ % 6 === 0) {
        void adapter
          .call("agents.setup", {})
          .then((rows) => live && setAgents(rows))
          .catch(() => live && setAgents((a) => a ?? []));
      }
      if (step === "devices" || step === "setup" || s.finishedLater) {
        void adapter
          .call("host.status", {})
          .then((h) => live && setHost(h))
          .catch(() => {});
      }
    };
    read(true);
    const focus = () => read(true);
    window.addEventListener("focus", focus);
    const timer = setInterval(() => read(), pollMs);
    return () => {
      live = false;
      clearInterval(timer);
      window.removeEventListener("focus", focus);
    };
  }, [active, adapter, step, s.finishedLater, tick, pollMs]);

  const rows: FirstRunAgent[] | null = agents
    ? agents.map((a) => ({ agent: a.agent, name: a.name, installed: a.installed, configured: a.configured, signIn: a.signIn ?? null, signedIn: Boolean(a.signedIn), mcpConfig: a.mcpConfig ?? null }))
    : null;

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

  const baseInput: FirstRunInput = {
    step,
    hostOs: hostOs(),
    agents: rows,
    agent: s.agent,
    picked: s.picked,
    keys: keysInput,
    agentSigningIn: signingAgent,
    agentSignInError: signInError,
    connecting: connecting !== null,
    connectingAgent: connecting,
    connectError,
    // No process check yet (see the module comment).
    agentOpenBeforeSetup: null,
    agentUsedAt: null,
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
    extra: withoutFix(extra),
    plan,
  };

  // The lead connected agent (the core's order), for the try screen's access log.
  const leadName = (() => {
    const v = firstRunView(core, { ...baseInput, step: "setup" });
    const t = v.setup?.prompt ? v.setup.title.replace(/^Try it in /, "").replace(/\.$/, "") : null;
    return t;
  })();
  // The first time the lead agent uses Cua since the try screen opened.
  const usedMs = s.usedAtMs ?? (s.setupStartedMs && leadName ? agentUsedMs(core, host, s.setupStartedMs, leadName) : null);
  useEffect(() => {
    if (usedMs && !s.usedAtMs) writeFirstRun({ usedAtMs: usedMs });
  }, [usedMs, s.usedAtMs]);
  // An agent's own sign-in finished (its file is there): stop waiting.
  useEffect(() => {
    if (signingAgent && agents?.some((a) => a.agent === signingAgent && a.signedIn)) setSigningAgent(null);
  }, [signingAgent, agents]);

  const input: FirstRunInput = { ...baseInput, agentUsedAt: usedMs ? clockTime(usedMs) : null };
  const view = withExtraFix(firstRunView(core, input), extra.fix ?? null);

  const welcome = settings.data?.telemetry
    ? onboardingView(core, { ...onboardingInitial(core, null), step: "welcome", telemetry: onboardingTelemetry(settings.data.telemetry) })
    : null;
  const usageNote: FirstRunUsage | null =
    step === "connect" && welcome?.usage && welcome.notice && welcome.noticeLinkLabel && welcome.noticeLinkUrl
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

  // The funnel's events, in the old flow's shape: started on the first two screens, left on the last two.
  const telemetryState = (): OnboardingFlowState => ({
    ...onboardingInitial(core, session.data?.identity ?? null),
    step: step === "welcome" || step === "connect" ? "welcome" : "agents",
    runCounted: true,
    telemetry: settings.data?.telemetry ? onboardingTelemetry(settings.data.telemetry) : null,
  });

  const refresh = () => setTick((n) => n + 1);

  const go = (next: FirstRunStep) => {
    writeFirstRun(next === "setup" ? { step: next, setupStartedMs: readFirstRun().setupStartedMs ?? Date.now() } : { step: next });
  };
  const forward = () => go(STEPS[Math.min(STEPS.indexOf(step) + 1, STEPS.length - 1)]!);

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

  // Signed in on a computer that isn't in the account yet: add it, once.
  const autoSetUp = useRef(false);
  useEffect(() => {
    if (!active || (step !== "devices" && step !== "setup") || !signedIn || !host || host.configured || autoSetUp.current) return;
    autoSetUp.current = true;
    void setUpThisComputer();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, step, signedIn, host?.configured]);

  const connect = async (agent: string) => {
    if (!adapter || connecting) return;
    setConnecting(agent);
    setConnectError(null);
    try {
      setAgents(await adapter.call("agents.configure", { agents: [agent] }));
      const was = readFirstRun().picked ?? [];
      writeFirstRun({ picked: was.includes(agent) ? was : [...was, agent] });
    } catch (e) {
      setConnectError(e instanceof Error ? e.message : String(e));
    } finally {
      setConnecting(null);
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

  const copyPrompt = async (shown?: string) => {
    const text = shown ?? view.setup?.prompt?.text ?? firstRunView(core, { ...input, step: "setup" }).setup?.prompt?.text ?? "";
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
    connect,
    signIn,
    copyPrompt,
    openUrl: (url) => session.openExternal(url).catch(() => {}),
    extra: extraAct,
    async act(id, settingsUrl) {
      switch (id) {
        case "start":
        case "continue":
        case "skip":
        case "not-now":
          forward();
          break;
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
        case "copy":
          await copyPrompt();
          break;
        case "add":
          await extraAct("add");
          break;
      }
      refresh();
    },
    async finish() {
      if (store) track(() => store.trackOnboardingFinished(telemetryState()));
      await session.completeOnboarding(host?.configured ? "host" : "client");
      writeFirstRun({ finishedLater: false, todosClosed: false });
    },
    later() {
      if (store) track(() => store.trackOnboardingSkipped(telemetryState()));
      store?.cancelSignIn();
      // The shell stops opening it (the sidebar offers it again).
      writeSavedOnboarding({ ...readSavedOnboarding(), skipped: true });
      writeFirstRun({ finishedLater: true });
    },
    back() {
      go(STEPS[Math.max(STEPS.indexOf(step) - 1, 0)]!);
    },
  };
}

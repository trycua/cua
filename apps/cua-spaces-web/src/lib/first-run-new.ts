// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run (CUA-3318, storyboard CUA-3193), phase 1: what each
 * screen says and which choices it offers, from what the app knows. Pure, so every state is tested
 * without a host. The page (`components/first-run-new`) draws it.
 *
 * 1 Combine all your computers into one supercomputer. Add this computer (sign in) or Skip for now.
 * 1b Finish signing in in your browser: Try again, Use a code instead; "Sign-in didn't finish". Once signed in, this
 *   computer joins the account here ("Adding this Mac to your account"), with Try again if that fails.
 *   There is no step to pick where agents work (Francesco, 8 Nov 2026): it confused people. Agents can use every
 *   ready computer in the account; Settings and the Computers page change that, and make a separate computer.
 * 2 On a Mac: let Cua Spaces see and use apps (Screen Recording and Accessibility, which Cua Driver
 *   uses too), each with Allow, or Later. `lib/app-permissions.ts` has its words. Left out when every one (and Local
 *   Network, where it's asked) is already on: the step would ask nothing.
 * 3 Which agents do you use? The coding agents found (all ticked, untick any to leave alone; Choose later skips it),
 *   each with where it is installed, or Install Claude Code / Install Codex instead. Kept in setup, not moved to the
 *   first job: that job is typed into the agent in the person's own terminal, so Cua has to be in the agent's settings
 *   before it, and there is no moment in the app to ask then. A job picks its own agent.
 *   Continue turns the screen into a checklist (CUA-4067, `connectPlan`): this computer set up to run jobs first, then
 *   for each agent Cua's tools, its skills, Cua Driver (on a Mac) and its sign-in saved to Keyvault, each row saying
 *   what is being done and where, live, with its own Retry.
 * 4 Ready: "<agents> can now use <computers>" (every ready computer), one line to try in their own terminal. Done
 *   ends setup on Home.
 *
 * The step dots under the page count exactly these steps (`runSteps`): one per step this run shows, so 4 on a Mac
 * that still needs a permission and 3 elsewhere.
 *
 * The rest (invite a teammate, add another computer, Cua from Terminal) is asked for on Home once the first job is
 * done, not during setup (CUA-3150, idea C): `bringRestView`. Letting agents use apps on this Mac moved into step 3's
 * checklist (CUA-4067), so Home no longer asks for it a second time.
 */

import type { Machine, SignInPhase } from "@/bridge";
import type { ConnectPart, ConnectResult } from "@/bridge/contracts/agents";

/**
 * `next`: an older run's last screen, saved before the rest moved to Home; such a run finishes on Home. `where`: an
 * older run saved on the removed "Pick where your agents work" step; such a run goes on from the step after sign-in.
 */
export type NewRunScreen = "welcome" | "signin" | "where" | "permissions" | "agent" | "ready" | "next";

/** The screens setup can show, in order (`RUN_STEPS` says which this run shows, and the step each belongs to). */
export const NEW_RUN_SCREENS: NewRunScreen[] = ["welcome", "signin", "permissions", "agent", "ready"];

/** One step of setup: one step dot, the screens it covers (the first is where its dot opens) and the name read out with it. */
export type RunStepId = "computer" | "permissions" | "agents" | "ready";
export interface RunStep {
  id: RunStepId;
  screens: readonly NewRunScreen[];
  name: string;
}

/**
 * Every step setup can have, in order. The step dots and where setup goes next are both read from this one list
 * (`runSteps`), so the dots always count the steps the run really shows. Adding this computer is one step: the
 * sign-in screen is the wait after its button, and an older run saved on the removed `where` step counts as it too.
 */
export const RUN_STEPS: readonly RunStep[] = [
  { id: "computer", screens: ["welcome", "signin", "where"], name: "Add this computer" },
  { id: "permissions", screens: ["permissions"], name: "Let Cua Spaces use apps" },
  { id: "agents", screens: ["agent"], name: "Which agents do you use?" },
  { id: "ready", screens: ["ready"], name: "Try your first job" },
];

/**
 * What decides which steps a run shows. `permissions`, on a Mac: `ask` when Cua Spaces still needs one, `skip` when
 * the app has none to give or every one is already on, null while that isn't known yet (the step is counted then).
 * Other computers have no permissions step.
 */
export interface RunFacts {
  os: string;
  permissions: "ask" | "skip" | null;
}

/** The steps this run shows, in order: the only list the step dots and the moves between steps read. */
export function runSteps(facts: RunFacts): RunStep[] {
  return RUN_STEPS.filter((s) => s.id !== "permissions" || (facts.os === "macos" && facts.permissions !== "skip"));
}

/** The step a screen belongs to; null for the old last screen (`next`), which finishes on Home. */
export function stepOf(screen: NewRunScreen): RunStepId | null {
  return RUN_STEPS.find((s) => s.screens.includes(screen))?.id ?? null;
}

/** Where setup goes once the step `screen` belongs to is done or skipped: the next step's first screen; null: setup ends. */
export function nextStepScreen(facts: RunFacts, screen: NewRunScreen): NewRunScreen | null {
  const steps = runSteps(facts);
  const id = stepOf(screen);
  const at = steps.findIndex((s) => s.id === id);
  if (at < 0) return null;
  return steps[at + 1]?.screens[0] ?? null;
}

/** What the run keeps between launches (inside the first run's saved record, so Reset & Show clears it). */
export interface NewRunSaved {
  screen: NewRunScreen;
  /** The coding agent Ready's line to try uses: the first one connected on screen 3 (setup id). */
  agent: string | null;
  /** The coding agents screen 3 connected (setup ids), in `CODING_AGENTS` order. */
  connected?: string[];
  /** The coding agents found on screen 3 that the person unticked, so they are left alone. */
  agentsOff?: string[];
  /** The furthest step reached, so a dot can go forward again only to a step already reached. */
  reachedStep?: RunStepId;
  /**
   * Whether this run shows the permissions step, decided when it leaves the first step (a Mac only), so the dots
   * don't change once the person turns the permissions on there.
   */
  permissions?: "ask" | "skip";
}

export const NEW_RUN_EMPTY: NewRunSaved = { screen: "welcome", agent: null };

/** "this Mac", "this PC", "this computer" (the app's THIS_MACHINE). */
export function placeName(os: string): string {
  return os === "macos" ? "this Mac" : os === "windows" ? "this PC" : "this computer";
}

/** "Mac mini", "Mac mini and Build server", "Mac mini, Gaming PC and Build server". */
export function listNames(names: string[]): string {
  return names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/* ---- 1 ---------------------------------------------------------------------------------- */

/**
 * The usage-data line on screen 1 (CUA-3551, option B): what is collected and a one-tap off, shown before anything
 * is sent. The hosts hold every usage event until the run leaves this screen, and drop them when it's turned off.
 * Built from the core's Welcome view (`onboarding.view`), so the words match Settings, Privacy.
 */
export interface WelcomeUsage {
  notice: string;
  label: string;
  linkLabel: string;
  linkUrl: string;
  on: boolean;
  /** False when the environment decides (Do Not Track, an environment variable, CI); `help` says which. */
  enabled: boolean;
  help: string | null;
}

export function welcomeUsage(
  view: {
    notice?: string | null;
    noticeLinkLabel?: string | null;
    noticeLinkUrl?: string | null;
    usage?: { label: string; on: boolean; enabled: boolean; help?: string | null } | null;
  } | null,
): WelcomeUsage | null {
  if (!view?.usage || !view.notice || !view.noticeLinkLabel || !view.noticeLinkUrl) return null;
  return {
    notice: view.notice,
    label: view.usage.label,
    linkLabel: view.noticeLinkLabel,
    linkUrl: view.noticeLinkUrl,
    on: view.usage.on,
    enabled: view.usage.enabled,
    help: view.usage.help ?? null,
  };
}

export const WELCOME = {
  title: "Combine all your computers into one supercomputer.",
  // The headline picked on 7 Nov 2026 (CUA-3753), then the one-liner and what to do here (CUA-3150).
  line: "Your computers, shared by your team and its agents. Sign in on each one to use them all from here.",
  primary: "Add this computer",
  skip: "Skip for now",
} as const;

/* ---- 1b --------------------------------------------------------------------------------- */

export interface SignInView {
  /** `adding`: signed in, and this computer is joining the account; `add-failed`: that didn't work. */
  state: "waiting" | "code" | "failed" | "adding" | "add-failed";
  title: string;
  line: string;
  /** The waiting row's words (none once it failed). */
  waiting: string | null;
  /** The code to enter, and the page to enter it on, when signing in with a code. */
  code: { code: string; url: string | null } | null;
  /** Which button is the white one: Try again only once sign-in (or adding this computer) failed. */
  tryAgainPrimary: boolean;
  /** Null while this computer is being added: there is nothing to try again yet. */
  tryAgain: string | null;
  useCode: string | null;
}

/**
 * The sign-in finished: this computer joins the account here, so setup has no separate step for it. Adding failed:
 * why, with Try again, or a code from another computer when the account wants proof it's the owner (`needsProof`).
 */
export function addingView(input: { os: string; state: "adding" | "failed"; error: string | null; needsProof?: boolean }): SignInView {
  const place = placeName(input.os);
  if (input.state === "adding")
    return {
      state: "adding",
      title: `Adding ${place} to your account`,
      line: "You're signed in. This takes a few seconds.",
      waiting: `Adding ${place}`,
      code: null,
      tryAgainPrimary: false,
      tryAgain: null,
      useCode: null,
    };
  return {
    state: "add-failed",
    title: `Couldn't add ${place} to your account`,
    line: input.needsProof && input.error ? `Confirm it's you. ${input.error}` : (input.error ?? "Something went wrong. Try again."),
    waiting: null,
    code: null,
    tryAgainPrimary: true,
    tryAgain: "Try again",
    useCode: input.needsProof ? "Use a code from another computer" : null,
  };
}

/** Why a sign-in didn't finish, in the words of #6408 for a browser that never confirmed; the core's own line otherwise. */
export function signInView(phase: SignInPhase, url: string | null): SignInView {
  const base = { tryAgain: "Try again", useCode: "Use a code instead" as string | null };
  if (phase.kind === "failed") {
    const line =
      !phase.reason || phase.reason === "notConfirmed"
        ? "The browser didn't confirm in time. Try again, or use a code if your browser is on another device."
        : phase.message;
    return { ...base, state: "failed", title: "Sign-in didn't finish", line, waiting: null, code: null, tryAgainPrimary: true };
  }
  if (phase.kind === "waiting" && phase.userCode) {
    return {
      ...base,
      state: "code",
      title: "Finish signing in in your browser",
      line: "Enter this code on the sign-in page, on any device.",
      waiting: "Waiting for the code",
      code: { code: phase.userCode, url },
      tryAgainPrimary: false,
      useCode: null,
    };
  }
  return {
    ...base,
    state: "waiting",
    title: "Finish signing in in your browser",
    line: "Create your account or sign in there. This window moves on when you're done.",
    waiting: "Waiting for your browser",
    code: null,
    tryAgainPrimary: false,
  };
}

/* ---- Where agents work ---------------------------------------------------------------- */

/**
 * The computers agents can use once setup is done: this one, then every other computer in the account that is
 * online and sharing (not offline, not stopped sharing, not a device of its own). There is no step to pick them
 * (Francesco, 8 Nov 2026); Settings and the Computers page change it.
 */
export function readyComputers(machines: readonly Pick<Machine, "name" | "online" | "current" | "device" | "notSharing">[] | null): string[] {
  return ["this", ...(machines ?? []).filter((m) => !m.current && !m.device && m.online && !m.notSharing).map((m) => m.name)];
}

/* ---- 3 ---------------------------------------------------------------------------------- */

/** The coding agents the first run looks for, in this order, with who makes them. */
export const CODING_AGENTS: { id: string; name: string; maker: string; command: string | null }[] = [
  { id: "claude-code", name: "Claude Code", maker: "By Anthropic", command: "claude" },
  { id: "codex", name: "Codex", maker: "By OpenAI", command: "codex" },
  { id: "gemini-cli", name: "Gemini CLI", maker: "By Google", command: "gemini" },
  { id: "opencode", name: "OpenCode", maker: "By SST", command: "opencode" },
  { id: "goose", name: "Goose", maker: "By Block", command: "goose" },
  { id: "hermes", name: "Hermes", maker: "By Nous Research", command: null },
];

export const agentInfo = (id: string) => CODING_AGENTS.find((a) => a.id === id) ?? { id, name: id, maker: "", command: null };

/** The official pages "Install" opens (phase 1: the page, not the installer). */
export const INSTALL_PAGES = { "claude-code": "https://claude.com/product/claude-code", codex: "https://developers.openai.com/codex" } as const;

export interface AgentView {
  state: "looking" | "found" | "none";
  title: string;
  line: string;
  /** Every agent found, ticked unless the person unticked it, with where it is installed ("On this Mac"). */
  agents: { id: string; name: string; where: string; picked: boolean }[];
  /** Under the list: each job picks its agent, and where to change this later. */
  note: string | null;
  continueEnabled: boolean;
  /** The quiet way past the step without connecting anything ("Choose later"); null while looking. */
  later: string | null;
  install: { primary: string; secondary: string; note: string } | null;
  error: string | null;
}

/** Why setup asks for a coding agent at all, said in every state of screen 3. */
export const NO_AI_OF_ITS_OWN = "Cua has no AI of its own.";

/** "On this Mac", "On this Mac and Mac mini": this computer first, then the account's other computers that have it. */
export function installedWhere(os: string, elsewhere: readonly string[] = []): string {
  return `On ${listNames([placeName(os), ...elsewhere])}`;
}

/**
 * Screen 3. `found`: the coding agents installed on this computer (setup ids), null while still looking; `off`: the
 * ones the person unticked; `elsewhere`: per agent, the account's other computers it is installed on, where the app
 * knows (it doesn't yet, so every row says "On this Mac"). Nothing has to be picked: every agent found starts ticked,
 * Continue connects the ticked ones (or, with none ticked, goes on like Choose later), and each job picks its own agent.
 */
export function agentView(input: {
  os: string;
  found: string[] | null;
  off?: readonly string[];
  elsewhere?: Readonly<Record<string, readonly string[]>>;
  error: string | null;
}): AgentView {
  const place = placeName(input.os);
  if (input.found === null)
    return {
      state: "looking",
      title: "Looking for your coding agents…",
      line: "",
      agents: [],
      note: null,
      continueEnabled: false,
      later: null,
      install: null,
      error: null,
    };
  const found = CODING_AGENTS.filter((a) => input.found!.includes(a.id));
  if (found.length === 0) {
    return {
      state: "none",
      title: "Install a coding agent",
      line: `${NO_AI_OF_ITS_OWN} We didn't find a coding agent on ${place}. Install Claude Code, then sign in to it once.`,
      agents: [],
      note: null,
      continueEnabled: false,
      later: "Install later",
      install: { primary: "Install Claude Code", secondary: "Install Codex instead", note: "Opens claude.com. Come back when it's installed." },
      error: null,
    };
  }
  const off = input.off ?? [];
  const one = found.length === 1 ? found[0]! : null;
  return {
    state: "found",
    title: "Which agents do you use?",
    line: one
      ? `${NO_AI_OF_ITS_OWN} ${one.name} does the work, on your own plan.`
      : `${NO_AI_OF_ITS_OWN} ${listNames(found.map((a) => a.name))} do the work, each on your own plan.`,
    agents: found.map((a) => ({ id: a.id, name: a.name, where: installedWhere(input.os, input.elsewhere?.[a.id]), picked: !off.includes(a.id) })),
    note: "You choose the agent each time you start a job. You can change this later in Settings.",
    continueEnabled: true,
    later: "Choose later",
    install: null,
    error: input.error,
  };
}

/** The agents screen 3 connects: every one found that the person didn't untick, in `CODING_AGENTS` order. */
export function chosenAgents(found: string[] | null, off: readonly string[] = []): string[] {
  return CODING_AGENTS.filter((a) => (found ?? []).includes(a.id) && !off.includes(a.id)).map((a) => a.id);
}

/** Ticks or unticks one agent on screen 3. */
export function toggleAgentOff(off: readonly string[], id: string): string[] {
  return off.includes(id) ? off.filter((x) => x !== id) : [...off, id];
}

/* ---- 3, after Continue: the checklist (CUA-4067) ------------------------------------------- */

/** A row of the checklist: this computer set up to run jobs, or one part of one agent's connection. */
export type ChecklistPart = "this-computer" | ConnectPart;

export interface ChecklistItem {
  /** `this-computer`, or `<agent>:<part>`. */
  key: string;
  agent: string | null;
  part: ChecklistPart;
}

/**
 * Where a row stands. `needs_code`: the agent signs in with a code made for Cua (Codex), which the row gets with
 * its own button. `skipped` rows (a part the agent doesn't have) are not drawn.
 */
export type ChecklistStatus = "waiting" | "running" | "done" | "needs_permission" | "needs_code" | "failed" | "skipped";

export interface ChecklistState {
  status: ChecklistStatus;
  /** Where it was written (`~/.claude.json`, `Keyvault`). */
  where: string | null;
  /** Why, when it failed or needs something. */
  line: string | null;
  /** The code to enter and the page to enter it on, while a code sign-in waits. */
  code?: { code: string; url: string } | null;
}

export const CHECKLIST_WAITING: ChecklistState = { status: "waiting", where: null, line: null };

/** The agents whose sign-in Connect saves to Keyvault (`signin::importable`). */
export const SIGN_IN_AGENTS: readonly string[] = ["claude-code", "codex"];

export const checklistKey = (agent: string | null, part: ChecklistPart) => (agent ? `${agent}:${part}` : part);

/**
 * The rows Continue runs, in order. This computer first (`thisComputer`: signed in and not yet able to run jobs):
 * setting it up restarts its Spaces service, so it runs and comes back online before anything else talks to it.
 * Then per agent: Cua's tools, its skills, Cua Driver (`driver`: a Mac whose app carries it), and last the sign-in,
 * which can take the longest (Claude Code may refresh one that ran out).
 */
export function connectPlan(input: { agents: readonly string[]; driver: boolean; thisComputer: boolean }): ChecklistItem[] {
  const items: ChecklistItem[] = input.thisComputer ? [{ key: "this-computer", agent: null, part: "this-computer" }] : [];
  for (const agent of input.agents) {
    const parts: ConnectPart[] = [
      "mcp",
      "skills",
      ...(input.driver ? (["driver"] as const) : []),
      ...(SIGN_IN_AGENTS.includes(agent) ? (["sign_in"] as const) : []),
    ];
    for (const part of parts) items.push({ key: checklistKey(agent, part), agent, part });
  }
  return items;
}

/** The `agents.connect` arguments that run one agent's row on its own (Continue runs them one by one; Retry runs one). */
export function connectArgs(agent: string, part: ConnectPart): { agents: string[]; signIn: boolean; skills: boolean; mcp: boolean; driver?: boolean } {
  switch (part) {
    case "mcp":
      return { agents: [agent], signIn: false, skills: false, mcp: true };
    case "skills":
      return { agents: [agent], signIn: false, skills: true, mcp: false };
    case "driver":
      return { agents: [agent], signIn: false, skills: false, mcp: false, driver: true };
    case "sign_in":
      // Cua's entry again (unchanged when it's there): a connect that touches the agent's settings may let Claude Code
      // refresh a sign-in that ran out, which a sign-in-only connect never does.
      return { agents: [agent], signIn: true, skills: false, mcp: true };
  }
}

/** A row's state from what `agents.connect` answered for it; null: the answer has no such part (a host without it). */
export function checklistFromResult(part: ConnectPart, result: ConnectResult | undefined): ChecklistState | null {
  if (!result) return { status: "failed", where: null, line: "Cua Spaces didn't answer. Try again." };
  const r = result.parts?.find((x) => x.part === part);
  if (r) {
    // Codex makes a sign-in for Cua with a code instead of sharing its own.
    if (part === "sign_in" && r.status === "failed" && result.sign_in?.refused === "not_yet") return { status: "needs_code", where: null, line: null };
    return { status: r.status, where: r.where, line: r.line };
  }
  if (result.parts) return null;
  // An older host: one answer for everything.
  if (part === "sign_in") {
    const s = result.sign_in;
    if (!s) return null;
    return s.imported ? { status: "done", where: "Keyvault", line: null } : { status: "failed", where: null, line: s.line };
  }
  if (part === "driver") return null;
  return result.tools_ok ? { status: "done", where: null, line: null } : { status: "failed", where: null, line: result.tools_line };
}

export interface ChecklistRowView {
  key: string;
  status: ChecklistStatus;
  /** What is done, in plain words, with where in brackets. */
  text: string;
  /** Under it: why it failed, what to allow, the code to enter, or what it means. */
  sub: string | null;
  /** The row's own button: Retry, Allow, Get a code, or the sign-in page again while a code waits. */
  action: { id: "retry" | "allow" | "code" | "open"; label: string } | null;
}

const inBrackets = (where: string | null) => (where ? ` (${where})` : "");

/** One row's words, from the part, the agent's name and where it stands. */
export function checklistRowView(item: ChecklistItem, state: ChecklistState, os: string): ChecklistRowView {
  const name = item.agent ? agentInfo(item.agent).name : "";
  const place = placeName(os);
  const done = state.status === "done";
  const failed = state.status === "failed";
  const retry = failed ? ({ id: "retry", label: "Retry" } as const) : null;
  const why = failed ? (state.line ?? "Something went wrong.") : null;
  const base = { key: item.key, status: state.status };
  switch (item.part) {
    case "this-computer": {
      const Place = place.charAt(0).toUpperCase() + place.slice(1);
      return {
        ...base,
        text: done ? `${Place} can run your agents' jobs` : failed ? `Couldn't set up ${place} to run jobs` : `Setting up ${place} to run your agents' jobs`,
        sub: why ?? `Shares ${place}'s desktop with Cua so jobs can run on it. You can turn this off in Settings.`,
        action: retry,
      };
    }
    case "mcp":
      return {
        ...base,
        text: done
          ? `Added Cua's tools to ${name}${inBrackets(state.where)}`
          : failed
            ? `Couldn't add Cua's tools to ${name}`
            : `Adding Cua's tools to ${name}${inBrackets(state.where)}`,
        sub: why,
        action: retry,
      };
    case "skills":
      return {
        ...base,
        text: done
          ? `Installed Cua's skills for ${name}${inBrackets(state.where)}`
          : failed
            ? `Couldn't install Cua's skills for ${name}`
            : `Installing Cua's skills for ${name}`,
        sub: why,
        action: retry,
      };
    case "driver":
      if (state.status === "needs_permission")
        return {
          ...base,
          text: `${name} needs your permission to use apps on ${place}`,
          sub: state.line ?? "Allow Screen Recording and Accessibility so agents can see and use apps.",
          action: { id: "allow", label: "Allow" },
        };
      return {
        ...base,
        text: done
          ? `Let ${name} use apps on ${place} (Cua Driver)`
          : failed
            ? `Couldn't let ${name} use apps on ${place}`
            : `Letting ${name} use apps on ${place} (Cua Driver)`,
        sub: why,
        action: retry,
      };
    case "sign_in": {
      const purpose = "so jobs on your other computers can use it";
      if (state.status === "needs_code")
        return {
          ...base,
          text: `Sign ${name} in for your other computers`,
          sub: state.code
            ? `Enter ${state.code.code} on the sign-in page. This row moves on when you're done.`
            : `${name} signs in with a code made for Cua. Your own sign-in stays as it is.`,
          action: state.code ? { id: "open", label: "Open the sign-in page" } : { id: "code", label: "Get a code" },
        };
      return {
        ...base,
        text: done
          ? `Saved your ${name} sign-in to Keyvault ${purpose}`
          : failed
            ? `Couldn't save your ${name} sign-in to Keyvault`
            : `Saving your ${name} sign-in to Keyvault ${purpose}`,
        sub: why,
        action: retry,
      };
    }
  }
}

/** Nothing is running or waiting any more: Continue can go on. */
export function checklistSettled(states: readonly ChecklistState[]): boolean {
  return states.every((s) => s.status !== "waiting" && s.status !== "running");
}

/** The agents Ready names: the ones whose tools went in (their `mcp` row is done or wasn't needed). */
export function connectedAgents(agents: readonly string[], states: Readonly<Record<string, ChecklistState>>): string[] {
  return agents.filter((a) => {
    const s = states[checklistKey(a, "mcp")]?.status;
    return s === "done" || s === "skipped";
  });
}

/** The checklist's heading and line, from where its rows stand. */
export function checklistView(states: readonly ChecklistState[]): { title: string; line: string } {
  if (!checklistSettled(states)) return { title: "Connecting your agents", line: "Each step shows here as it happens." };
  const stuck = states.filter((s) => s.status === "failed" || s.status === "needs_permission" || s.status === "needs_code").length;
  if (stuck === 0) return { title: "Your agents are connected", line: "Everything below is done." };
  return {
    title: "Almost there",
    line: stuck === 1 ? "One step needs you. You can also do it later in Settings." : `${stuck} steps need you. You can also do them later in Settings.`,
  };
}

/* ---- 4 ---------------------------------------------------------------------------------- */

export interface ReadyView {
  title: string;
  line: string;
  command: string;
  copy: string;
  done: string;
}

/** From this many computers on, Ready counts the others instead of naming each ("this Mac and 5 other computers"). */
export const READY_NAMES_AT_MOST = 4;

/**
 * `computers`: the ready computers' names (`readyComputers`), this computer first as `this` (written "this Mac"). `canMakeSeparate`:
 * false where this computer can't make a separate computer, so the line to try never asks for one (CUA-3150, idea D).
 */
export function readyView(input: { os: string; agent: string; agents?: string[]; computers: string[]; canMakeSeparate?: boolean | null }): ReadyView {
  const place = placeName(input.os);
  const a = agentInfo(input.agent);
  // Every agent connected is named; the line to try uses `agent`.
  const connected = input.agents?.length ? input.agents : [input.agent];
  const all = input.computers.map((c) => (c === "this" ? place : c));
  const names = all.length > READY_NAMES_AT_MOST ? [all[0]!, `${all.length - 1} other computers`] : all;
  const elsewhere = input.computers.find((c) => c !== "this");
  const task = elsewhere
    ? `Build this branch on ${elsewhere}`
    : input.canMakeSeparate === false
      ? `Run this repo's tests on ${place} and tell me what fails`
      : `Make a separate Linux computer on ${place} and run this repo's tests in it`;
  const command = a.command ? `${a.command} "${task}"` : task;
  return {
    title: `${listNames(connected.map((id) => agentInfo(id).name))} can now use ${listNames(names)}`,
    line: a.command ? "Try it from your own terminal. Nothing runs until you do." : `Paste this into ${a.name}. Nothing runs until you do.`,
    command,
    copy: "Copy",
    done: "Done",
  };
}

/* ---- After the first job: the rest, on Home ------------------------------------------------ */

export interface BringRestView {
  title: string;
  line: string;
  rows: { id: "invite" | "add-computer" | "cli"; title: string; sub: string }[];
  dismiss: string;
}

/**
 * The extra setup, asked for on Home only once the first job is done (CUA-3150, idea C), never during setup.
 * `cli`: this app carries the `cua` command and it isn't on this computer yet (the host answered `cli.plan`); the
 * row is for people who type commands, so it costs nobody else a step (CUA-3551, option B). Letting agents use apps
 * on this computer is not a row: setup's agents step connects Cua Driver (CUA-4067), and a permission still off
 * shows in Home's apps card (`AppsOnThisMac`).
 */
export function bringRestView(input: { os: string; cli?: boolean }): BringRestView {
  return {
    title: "Now bring the rest in",
    line: "Your first job is done.",
    rows: [
      { id: "invite", title: "Invite a teammate", sub: "They can use these computers and agents too." },
      { id: "add-computer", title: "Add another computer", sub: "Install Cua Spaces on it and sign in." },
      ...(input.cli ? [{ id: "cli" as const, title: "Use Cua from Terminal", sub: "Adds the cua command." }] : []),
    ],
    dismiss: "Not now",
  };
}

/** Where the offer stands: `waiting` from the end of setup until the first job is done, `shown` from then until "Not now". */
export type RestOffer = "waiting" | "shown" | "dismissed";

/** A job's end as Home can see it: one of its tasks, or an agent run in one of your Spaces. */
export interface JobSignals {
  /** Home's tasks (they only start after setup): how each stands (`taskState`, or null when its run isn't read). */
  tasks: { state: string | null; ending: string | null }[];
  /** Agent runs in your running Spaces: when each started, and its status. */
  runs: { startedMs: number | null; status: string | null }[];
}

/**
 * The first job is done: a Home task that finished (done, or its computer kept or thrown away; a chat continued
 * from the Ready screen is one too), or an agent run started since setup ended (`sinceMs`) that finished its turn (`idle`, waiting for the next message). A run that stopped
 * with an error doesn't count: the offer waits for a job that worked.
 */
export function firstJobDone(signals: JobSignals, sinceMs: number): boolean {
  const taskDone = signals.tasks.some((t) => t.ending === "kept" || t.ending === "thrown" || t.state === "done" || t.state === "kept" || t.state === "thrown");
  const runDone = signals.runs.some((r) => r.status === "idle" && r.startedMs !== null && r.startedMs >= sinceMs);
  return taskDone || runDone;
}

/** The order of two steps in `RUN_STEPS` (a step left out of this run keeps its place). */
const stepIndex = (id: RunStepId | null | undefined) => (id ? RUN_STEPS.findIndex((s) => s.id === id) : -1);

/** The furthest step a run has reached, once it's on `screen`. Never goes back. */
export function furthestStep(reached: RunStepId | undefined, screen: NewRunScreen): RunStepId | undefined {
  const here = stepOf(screen) ?? undefined;
  return stepIndex(here) > stepIndex(reached) ? here : reached;
}

export interface StepDot {
  /** "Step 3 of 4: Which agents do you use?". */
  label: string;
  current: boolean;
  /** The screen a click opens; null: it can't be opened from here (a step not reached yet, or the one showing). */
  to: NewRunScreen | null;
}

/**
 * The step dots on `screen`: one per step this run shows (`runSteps`), the current one lit. A dot opens any earlier
 * step, and a later step only once the run has reached it, so a dot never skips the sign-in. On the sign-in, the
 * first dot goes back to its start. Null on a screen that isn't a step (`next`).
 */
export function stepDots(facts: RunFacts, screen: NewRunScreen, reached?: RunStepId): StepDot[] | null {
  const steps = runSteps(facts);
  const at = steps.findIndex((s) => s.screens.includes(screen));
  if (at < 0) return null;
  const far = Math.max(stepIndex(reached), stepIndex(steps[at]!.id));
  return steps.map((step, i) => {
    const to = step.screens[0]!;
    const open = to !== screen && (i <= at || stepIndex(step.id) <= far);
    return { label: `Step ${i + 1} of ${steps.length}: ${step.name}`, current: i === at, to: open ? to : null };
  });
}

/** The step an arrow key moves to from the dot at `from`: the nearest one in that direction that can be opened. */
export function dotStep(dots: readonly StepDot[], from: number, dir: -1 | 1): number | null {
  for (let i = from + dir; i >= 0 && i < dots.length; i += dir) if (dots[i]!.to) return i;
  return null;
}

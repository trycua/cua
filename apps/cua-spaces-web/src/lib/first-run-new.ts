// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The new first run (Labs, "Try the new setup"; CUA-3318, storyboard CUA-3193), phase 1: what each
 * screen says and which choices it offers, from what the app knows. Pure, so every state is tested
 * without a host. The page (`components/first-run-new`) draws it.
 *
 * 1 Bring all your computers under one account: Add this computer (sign in) or Skip for now.
 * 1b Finish signing in in your browser: Try again, Use a code instead; "Sign-in didn't finish".
 * 2 Pick where your agents work: this Mac, the account's other computers (offline ones can't be
 *   picked), Add another computer you own, and a separate computer on this Mac.
 * 3 The coding agent found on this Mac, or Install Claude Code / Install Codex instead.
 * 4 Ready: "<agent> can now use <computers>", one line to try in their own terminal.
 * 6 Next steps: invite a teammate, add another computer, let agents use apps on this Mac.
 */

import type { Machine, SignInPhase } from "@/bridge";
import { relativeTime } from "@/lib/utils";

export type NewRunScreen = "welcome" | "signin" | "where" | "agent" | "ready" | "next";

export const NEW_RUN_SCREENS: NewRunScreen[] = ["welcome", "signin", "where", "agent", "ready", "next"];

/** What the run keeps between launches (inside the first run's saved record, so Reset & Show clears it). */
export interface NewRunSaved {
  screen: NewRunScreen;
  /** The computers picked on screen 2: `this` for this computer, else a machine id or the separate computer's Space id. */
  picks: string[];
  /** The coding agent picked on screen 3 (setup id). */
  agent: string | null;
  /** This run asked to add this computer and the sign-in finished: its row says "just now". */
  addedInRun: boolean;
  /** The separate computer's create (its pending row id), once asked for. */
  separatePendingId: string | null;
}

export const NEW_RUN_EMPTY: NewRunSaved = { screen: "welcome", picks: ["this"], agent: null, addedInRun: false, separatePendingId: null };

/** "this Mac", "this PC", "this machine" (the app's THIS_MACHINE). */
export function placeName(os: string): string {
  return os === "macos" ? "this Mac" : os === "windows" ? "this PC" : "this machine";
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** "Mac mini", "Mac mini and Build server", "Mac mini, Gaming PC and Build server". */
export function listNames(names: string[]): string {
  return names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/* ---- 1 ---------------------------------------------------------------------------------- */

export const WELCOME = {
  title: "Bring all your computers under one account",
  line: "Sign in on each Mac, PC or Linux machine you own. Then you, your team and your coding agents can use any of them from here.",
  primary: "Add this computer",
  skip: "Skip for now",
} as const;

/* ---- 1b --------------------------------------------------------------------------------- */

export interface SignInView {
  state: "waiting" | "code" | "failed";
  title: string;
  line: string;
  /** The waiting row's words (none once it failed). */
  waiting: string | null;
  /** The code to enter, and the page to enter it on, when signing in with a code. */
  code: { code: string; url: string | null } | null;
  /** Which button is the white one: Try again only once sign-in failed. */
  tryAgainPrimary: boolean;
  tryAgain: string;
  useCode: string | null;
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
      line: "Open the sign-in page on any device and enter this code. This window moves on by itself when you're done.",
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
    line: "Cua opened in your browser. Create your account there, or sign in if you already have one. This window moves on by itself when you're done.",
    waiting: "Waiting for your browser",
    code: null,
    tryAgainPrimary: false,
  };
}

/* ---- 2 ---------------------------------------------------------------------------------- */

export interface ComputerRow {
  id: string;
  name: string;
  os: "macos" | "windows" | "linux" | null;
  sub: string;
  picked: boolean;
  /** Offline: shown greyed, with no check circle, and it can't be picked. */
  disabled: boolean;
}

export interface WhereView {
  title: string;
  line: string;
  thisRow: {
    name: string;
    /** "Added to your account just now", "In your account", "not added yet" ... */
    status: string;
    /** The white tick before the status (added in this run). */
    tick: boolean;
    model: string | null;
    /** Signed out, the frame reads "MacBook Pro · not added yet": the model comes first. */
    modelFirst: boolean;
    picked: boolean;
    /** Signed out: Add (opens 1b). Adding failed: Try again. */
    action: { id: "add" | "retry-add"; label: string } | null;
  };
  /** Signed in: the account's other computers (empty: `emptyLine`). Signed out: null, and `signInLine` shows instead. */
  others: ComputerRow[] | null;
  othersLabel: string;
  emptyLine: string;
  signInLine: { link: string; rest: string } | null;
  /** Signed in only: it needs an account. */
  addAnother: { title: string; sub: string } | null;
  separate: { title: string; sub: string; disabled: boolean; state: "offer" | "checking" | "explain" | "making" | "ready" | "failed"; action: string | null };
  continueEnabled: boolean;
}

export interface WhereInput {
  os: string;
  signedIn: boolean;
  /** This computer: being added, added (`addedInRun`: in this run), or the add failed. */
  thisState: "adding" | "added" | "failed" | "signed-out";
  addedInRun: boolean;
  addError: string | null;
  model: string | null;
  machines: Pick<Machine, "id" | "name" | "os" | "online" | "lastSeen" | "current" | "device" | "notSharing">[] | null;
  picks: string[];
  /** Whether this computer can make a separate computer: null while still checking. */
  canMakeSeparate: boolean | null;
  /** Why not, in the host's words (used where the frame's sentence doesn't fit: not a Mac). */
  separateBlocked: string | null;
  separate: { phase: "none" | "explain" | "making" | "ready" | "failed"; name: string | null; error: string | null };
  now: number;
}

function machineOs(os: string): ComputerRow["os"] {
  const o = os.toLowerCase();
  if (o === "macos" || o === "darwin" || o === "mac") return "macos";
  if (o === "windows" || o === "win32") return "windows";
  if (o === "linux") return "linux";
  return null;
}

const OS_WORD: Record<NonNullable<ComputerRow["os"]>, string> = { macos: "macOS", windows: "Windows", linux: "Linux" };

/** The account's other computers: ones the app can reach are offered, offline ones shown greyed. */
export function otherComputers(input: Pick<WhereInput, "machines" | "picks" | "now">): ComputerRow[] {
  return (input.machines ?? [])
    .filter((m) => !m.current && !m.device)
    .map((m) => {
      const os = machineOs(m.os);
      const offline = !m.online;
      const sub = offline
        ? m.lastSeen
          ? `Offline, last seen ${relativeTime(m.lastSeen * 1000, input.now)}`
          : "Offline"
        : m.notSharing
          ? "Not sharing"
          : `${os ? OS_WORD[os] : "Computer"} · ready`;
      const disabled = offline || Boolean(m.notSharing);
      return { id: m.id, name: m.name, os, sub, picked: !disabled && input.picks.includes(m.id), disabled };
    });
}

export function whereView(input: WhereInput): WhereView {
  const place = placeName(input.os);
  const Place = cap(place);
  const thisPicked = input.picks.includes("this");
  const separate = (() => {
    const title = `Or make a separate computer on ${place}`;
    if (input.canMakeSeparate === false) {
      const sub =
        input.os === "macos"
          ? "This Mac can't make one. Older Intel Macs can't, for example. Your agents can still work on this Mac or on another computer you add."
          : `${input.separateBlocked ? `${input.separateBlocked} ` : `${Place} can't make one. `}Your agents can still work on ${place} or on another computer you add.`;
      return { title, sub, disabled: true, state: "offer" as const, action: null };
    }
    const s = input.separate;
    if (s.phase === "explain")
      return {
        title,
        sub: "If macOS asks to let Cua Spaces find devices on your local network, choose Allow. Cua uses it to reach the new computer.",
        disabled: false,
        state: "explain" as const,
        action: "Make it",
      };
    if (s.phase === "making") return { title, sub: `Making ${s.name ?? "it"} now. You can continue; it keeps going.`, disabled: false, state: "making" as const, action: null };
    if (s.phase === "ready") return { title, sub: `${s.name ?? "It"} is ready.`, disabled: false, state: "ready" as const, action: null };
    if (s.phase === "failed") return { title, sub: `Couldn't make it. ${s.error ?? ""}`.trim(), disabled: false, state: "failed" as const, action: "Try again" };
    if (input.canMakeSeparate === null) return { title, sub: "Checking whether this computer can make one…", disabled: true, state: "checking" as const, action: null };
    return { title, sub: "A Linux computer just for your agents, away from your files. About 2 minutes.", disabled: false, state: "offer" as const, action: null };
  })();

  if (!input.signedIn) {
    return {
      title: "Pick where your agents work",
      line: `Your agents can work on ${place} without an account. Add ${place} to an account when you want them on your other computers too.`,
      thisRow: { name: Place, status: "not added yet", tick: false, model: input.model, modelFirst: true, picked: thisPicked, action: { id: "add", label: "Add" } },
      others: null,
      othersLabel: "Other computers in your account",
      emptyLine: "",
      signInLine: { link: "Sign in", rest: " to see your other computers." },
      addAnother: null,
      separate,
      continueEnabled: input.picks.length > 0,
    };
  }
  const status =
    input.thisState === "adding"
      ? "Adding to your account…"
      : input.thisState === "failed"
        ? `Couldn't add it to your account.${input.addError ? ` ${input.addError}` : ""}`
        : input.addedInRun
          ? "Added to your account just now"
          : "In your account";
  return {
    title: "Pick where your agents work",
    line: `You're signed in, and ${place} is now in your account. Your agents can start here, or on your other computers while you keep working on this one.`,
    thisRow: {
      name: Place,
      status,
      tick: input.thisState === "added",
      model: input.model,
      modelFirst: false,
      picked: thisPicked,
      action: input.thisState === "failed" ? { id: "retry-add", label: "Try again" } : null,
    },
    others: otherComputers(input),
    othersLabel: "Other computers in your account",
    emptyLine: "None yet. Computers you or your team sign in on show up here.",
    signInLine: null,
    addAnother: { title: "Add another computer you own", sub: "Install Cua Spaces on it and sign in. It shows up here." },
    separate,
    continueEnabled: input.picks.length > 0,
  };
}

/** Picks a computer, or unpicks it. Offline computers never change. */
export function togglePick(picks: string[], id: string, rows: ComputerRow[]): string[] {
  if (rows.find((r) => r.id === id)?.disabled) return picks;
  return picks.includes(id) ? picks.filter((p) => p !== id) : [...picks, id];
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

const COUNT = ["", "one", "two", "three", "four", "five", "six"];

export interface AgentView {
  state: "looking" | "one" | "several" | "none";
  title: string;
  line: string;
  /** "Found on this Mac". */
  label: string;
  agents: { id: string; name: string; maker: string; picked: boolean }[];
  continueEnabled: boolean;
  install: { primary: string; secondary: string; note: string } | null;
  error: string | null;
}

/** `found`: the coding agents installed on this computer (setup ids); null while still looking. */
export function agentView(input: { os: string; found: string[] | null; picked: string | null; error: string | null }): AgentView {
  const place = placeName(input.os);
  const label = `Found on ${place}`;
  if (input.found === null)
    return { state: "looking", title: "Looking for your coding agent…", line: "", label, agents: [], continueEnabled: false, install: null, error: null };
  const found = CODING_AGENTS.filter((a) => input.found!.includes(a.id));
  if (found.length === 0) {
    return {
      state: "none",
      title: "Install a coding agent",
      line: `Cua doesn't come with its own, and we didn't find one on ${place}. Install Claude Code, then sign in to it once.`,
      label,
      agents: [],
      continueEnabled: false,
      install: { primary: "Install Claude Code", secondary: "Install Codex instead", note: "This opens the official Claude Code install page in your browser. Come back here when it's installed." },
      error: null,
    };
  }
  if (found.length === 1) {
    const a = found[0]!;
    return {
      state: "one",
      title: `Your agents will use ${a.name}`,
      line: `We found ${a.name} on ${place}. Your agents will use it to do the work while you keep working on other things.`,
      label,
      agents: [{ id: a.id, name: a.name, maker: a.maker, picked: true }],
      continueEnabled: true,
      install: null,
      error: input.error,
    };
  }
  const picked = found.some((a) => a.id === input.picked) ? input.picked : null;
  return {
    state: "several",
    title: "Pick the coding agent to use",
    line: `We found ${COUNT[found.length] ?? found.length} on ${place}. Pick the one your agents should use. You can change it for any job later.`,
    label,
    agents: found.map((a) => ({ id: a.id, name: a.name, maker: a.maker, picked: a.id === picked })),
    continueEnabled: picked !== null,
    install: null,
    error: input.error,
  };
}

/** The agent screen 3 settles on: the only one found, or the one picked among several. */
export function chosenAgent(found: string[] | null, picked: string | null): string | null {
  const list = CODING_AGENTS.filter((a) => (found ?? []).includes(a.id));
  if (list.length === 1) return list[0]!.id;
  return list.some((a) => a.id === picked) ? picked : null;
}

/* ---- 4 ---------------------------------------------------------------------------------- */

export interface ReadyView {
  title: string;
  line: string;
  command: string;
  copy: string;
  done: string;
}

/** `computers`: the picked computers' names, this computer first as `this` (written "this Mac"). */
export function readyView(input: { os: string; agent: string; computers: string[] }): ReadyView {
  const place = placeName(input.os);
  const a = agentInfo(input.agent);
  const names = input.computers.map((c) => (c === "this" ? place : c));
  const elsewhere = input.computers.find((c) => c !== "this");
  const task = elsewhere ? `Build this branch on ${elsewhere}` : `Make a separate Linux computer on ${place} and run this repo's tests in it`;
  const command = a.command ? `${a.command} "${task}"` : task;
  return {
    title: `${a.name} can now use ${listNames(names)}`,
    line: a.command ? "Try it from your own terminal. Nothing runs until you do." : `Paste this into ${a.name}. Nothing runs until you do.`,
    command,
    copy: "Copy",
    done: "Done",
  };
}

/* ---- 6 ---------------------------------------------------------------------------------- */

export interface NextView {
  title: string;
  line: string;
  rows: { id: "invite" | "add-computer" | "apps"; title: string; sub: string }[];
  done: string;
}

export function nextView(input: { os: string }): NextView {
  const place = placeName(input.os);
  const apps = input.os === "macos";
  return {
    title: "Now bring the rest in",
    line: apps
      ? `When you're ready, add your team and your other computers, and let agents use apps on ${place}.`
      : "When you're ready, add your team and your other computers.",
    rows: [
      { id: "invite", title: "Invite a teammate", sub: "They can use these computers and agents too." },
      { id: "add-computer", title: "Add another computer", sub: "Install Cua Spaces on it and sign in." },
      ...(apps ? [{ id: "apps" as const, title: `Let agents use apps on ${place}`, sub: "They can see the screen and use the mouse and keyboard, after you allow it." }] : []),
    ],
    done: "Done",
  };
}

/** Which progress dot is lit (screens 1 to 3 have dots; 4 and 6 none). */
export function dotFor(screen: NewRunScreen): number | null {
  return screen === "welcome" || screen === "signin" ? 0 : screen === "where" ? 1 : screen === "agent" ? 2 : null;
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The This Mac prompts and the sign-in prompt. The moments, rules and words are the app
 * core's (`nudge.secondAgent`, `nudge.riskyAction`,
 * `cua-spaces-app-core/src/nudges.rs`); this reads what they need, keeps
 * what the page remembers, and draws nothing.
 *
 * - A second agent: this Mac's access log (`host.status` → `recentAccess`),
 *   read every {@link SECOND_AGENT_POLL_MS}. Shown at most once a day per
 *   computer; "Don't ask again" turns it off for that computer.
 * - A risky action: the requests `cua mcp` holds (`nudges.pending`), read
 *   every {@link RISKY_POLL_MS} where the host answers it.
 *
 * - Sign in, once a Space is ready (`nudge.signIn`, CUA-2062): signed out,
 *   and a Space is running. Asked once per computer; "Sign in" starts the
 *   usual browser sign-in. Counted as `signin_prompt_shown`, `_accepted`
 *   and `_dismissed`.
 *
 * "Add a Space" and "Use a separate Space" start the same one-click create
 * the first run offers ({@link useAddSpaceWhenReady} over
 * `useFirstSpaceOffers`): macOS first, Linux when this Mac can't make
 * macOS, in the background. Nothing waits on it.
 */

import { useContext, useEffect, useRef, useState } from "react";

import { isUnsupported } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { HostAccess } from "./contracts/host";
import { useSession, useSpaces } from "./hooks";
import { useFirstSpaceOffers } from "./new-space-hooks";
import type { RiskyChoice, RiskyRequest } from "./ops/nudges";

/** `nudges::Nudge`. */
export interface Nudge {
  kind: "second-agent" | "risky-action" | "sign-in";
  key: string;
  eyebrow: string;
  title: string;
  body: string;
  note: string | null;
  primary: string;
  secondary: string;
  never: string | null;
}

/** `nudges::NudgeMemory`. */
interface NudgeMemory {
  shownDay: string | null;
  off: boolean;
}

export const SECOND_AGENT_POLL_MS = 15_000;
export const RISKY_POLL_MS = 1_000;

const memoryKey = (computer: string) => `cua.nudge.secondAgent.${computer}`;

function readMemory(computer: string): NudgeMemory {
  try {
    const raw = JSON.parse(globalThis.localStorage?.getItem(memoryKey(computer)) ?? "null") as Partial<NudgeMemory> | null;
    return { shownDay: typeof raw?.shownDay === "string" ? raw.shownDay : null, off: raw?.off === true };
  } catch {
    return { shownDay: null, off: false };
  }
}

function writeMemory(computer: string, memory: NudgeMemory): void {
  try {
    globalThis.localStorage?.setItem(memoryKey(computer), JSON.stringify(memory));
  } catch {
    // Not kept: it may show again tomorrow, never twice today in this window.
  }
}

/** The local day, `2026-10-18`. */
export function localDay(at = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${at.getFullYear()}-${p(at.getMonth() + 1)}-${p(at.getDate())}`;
}

/** Forget what was shown and turned off (tests). */
export function resetNudgeMemory(computer: string): void {
  try {
    globalThis.localStorage?.removeItem(memoryKey(computer));
  } catch {
    // Nothing kept.
  }
}

/**
 * Starts a Space on this Mac the way the first run's one-click offer does,
 * once this Mac's options are in: macOS when it can make it, else Linux.
 * Mount it only while a Space is wanted (it loads the host's options).
 * `onDone` gets the OS started, or null when this Mac can make neither now.
 */
export function useAddSpaceWhenReady(onDone: (os: "macos" | "linux" | null) => void): void {
  const first = useFirstSpaceOffers();
  const { createSpace } = useSpaces();
  const started = useRef(false);
  useEffect(() => {
    if (started.current) return;
    const offer = first.offers.find((o) => o.os === "macos" && !o.blocked) ?? first.offers.find((o) => o.os === "linux" && !o.blocked);
    if (offer) {
      started.current = true;
      // The create runs in the background service; its row shows progress.
      createSpace({ ...offer.args, os: offer.os }).catch(() => undefined);
      onDone(offer.os);
    } else if (first.known) {
      started.current = true;
      onDone(null);
    }
  }, [first.offers, first.known, createSpace, onDone]);
}

export interface SecondAgentNudge {
  nudge: Nudge | null;
  /** Closes it; the caller starts the Space ({@link useAddSpaceWhenReady}). */
  addSpace(): void;
  keepSharing(): void;
  dontAskAgain(): void;
}

/** The second-agent prompt for this Mac, when it is the moment. */
export function useSecondAgentNudge(): SecondAgentNudge {
  const ctx = useContext(BridgeContext);
  const adapter = ctx?.adapter ?? null;
  const core = ctx?.core;
  const [shown, setShown] = useState<{ nudge: Nudge; computer: string } | null>(null);
  const shownRef = useRef(shown);
  shownRef.current = shown;

  useEffect(() => {
    if (!adapter || !core) return;
    let live = true;
    const read = () => {
      if (shownRef.current) return;
      adapter
        .call("host.status", {})
        .then((status) => {
          if (!live || !status || shownRef.current) return;
          const computer = status.name?.trim() || "this-mac";
          const today = localDay();
          const recentAccess: HostAccess[] = status.recentAccess ?? [];
          const nudge = core.tryCall<Nudge | null>("nudge.secondAgent", { recentAccess, nowMs: Date.now(), today, memory: readMemory(computer) }) ?? null;
          if (!nudge) return;
          // Once a day: counted when it shows, whatever the answer.
          writeMemory(computer, { ...readMemory(computer), shownDay: today });
          setShown({ nudge, computer });
        })
        .catch(() => undefined);
    };
    read();
    const timer = setInterval(read, SECOND_AGENT_POLL_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [adapter, core]);

  return {
    nudge: shown?.nudge ?? null,
    addSpace: () => setShown(null),
    keepSharing: () => setShown(null),
    dontAskAgain: () => {
      if (shown) writeMemory(shown.computer, { ...readMemory(shown.computer), off: true });
      setShown(null);
    },
  };
}

export interface RiskyNudge {
  nudge: Nudge | null;
  /** "Use a separate Space": the call doesn't run here; the caller starts
   * the Space ({@link useAddSpaceWhenReady}). */
  separate(): void;
  /** "Run it here". */
  here(): void;
}

/** The risky-action question, while `cua mcp` waits for an answer. */
export function useRiskyNudge(): RiskyNudge {
  const ctx = useContext(BridgeContext);
  const adapter = ctx?.adapter ?? null;
  const core = ctx?.core;
  const [request, setRequest] = useState<RiskyRequest | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const answered = useRef(new Set<string>());

  useEffect(() => {
    if (!adapter) return;
    let live = true;
    let timer: ReturnType<typeof setInterval> | undefined;
    const read = () => {
      setNow(Date.now());
      adapter
        .call("nudges.pending", {})
        .then((pending) => {
          if (!live) return;
          setRequest(pending.find((r) => !answered.current.has(r.id) && r.expiresMs > Date.now()) ?? null);
        })
        .catch((e: unknown) => {
          // A host without the question never asks again this session.
          if (isUnsupported(e) && timer) clearInterval(timer);
        });
    };
    timer = setInterval(read, RISKY_POLL_MS);
    read();
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [adapter]);

  const nudge = request && core ? (core.tryCall<Nudge | null>("nudge.riskyAction", { request, nowMs: now }) ?? null) : null;
  const answer = (choice: RiskyChoice) => {
    if (!request || !adapter) return;
    answered.current.add(request.id);
    void adapter.call("nudges.answer", { id: request.id, choice }).catch(() => undefined);
    setRequest(null);
  };
  return { nudge, separate: () => answer("separate"), here: () => answer("here") };
}

const SIGN_IN_KEY = "cua.nudge.signIn";

function signInShown(): boolean {
  try {
    return (JSON.parse(globalThis.localStorage?.getItem(SIGN_IN_KEY) ?? "null") as { shown?: unknown } | null)?.shown === true;
  } catch {
    return false;
  }
}

function markSignInShown(): void {
  try {
    globalThis.localStorage?.setItem(SIGN_IN_KEY, JSON.stringify({ shown: true }));
  } catch {
    // Not kept: it may ask once more in a later window.
  }
}

export interface SignInNudge {
  nudge: Nudge | null;
  /** "Sign in": starts the browser sign-in and closes the prompt. */
  signIn(): void;
  /** "Not now". */
  notNow(): void;
}

/** The prompt to sign in, once a Space is ready and the user is signed out.
 * `waiting`: another prompt is open; this one waits its turn. */
export function useSignInNudge(waiting = false): SignInNudge {
  const ctx = useContext(BridgeContext);
  const core = ctx?.core;
  const store = ctx?.store ?? null;
  const session = useSession();
  const spaces = useSpaces();
  const [shown, setShown] = useState<Nudge | null>(null);
  const asked = useRef(false);

  const data = session.data;
  const ready = (spaces.data ?? []).find((sp) => sp.status === "running") ?? null;
  const signingIn = data?.signIn.kind === "starting" || data?.signIn.kind === "waiting";

  useEffect(() => {
    if (waiting || !core || !data || asked.current || shown) return;
    const nudge =
      core.tryCall<Nudge | null>("nudge.signIn", {
        input: {
          signedIn: data.signedIn || Boolean(data.identity),
          signingIn,
          canSignIn: true,
          spaceReady: Boolean(ready),
          spaceName: ready?.name ?? null,
          memory: { shown: signInShown() },
        },
      }) ?? null;
    if (!nudge) return;
    asked.current = true;
    // Once per computer: counted when it shows, whatever the answer.
    markSignInShown();
    void store?.telemetry.track([{ type: "step", step: "signin_prompt_shown", ok: true }]);
    setShown(nudge);
  }, [waiting, core, store, data, signingIn, ready, shown]);

  // Signed in some other way while it is open: it goes.
  useEffect(() => {
    if (shown && data && (data.signedIn || data.identity)) setShown(null);
  }, [shown, data]);

  const answer = (step: "signin_prompt_accepted" | "signin_prompt_dismissed") => {
    void store?.telemetry.track([{ type: "step", step, ok: true }]);
    setShown(null);
  };
  return {
    nudge: shown,
    signIn: () => {
      answer("signin_prompt_accepted");
      void session.signIn().catch(() => undefined);
    },
    notNow: () => answer("signin_prompt_dismissed"),
  };
}

/** Forget that the sign-in prompt was shown (tests). */
export function resetSignInNudge(): void {
  try {
    globalThis.localStorage?.removeItem(SIGN_IN_KEY);
  } catch {
    // Nothing kept.
  }
}

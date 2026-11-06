// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Why a computer did not answer, in plain words. Home names the computer and the reason (it needs an update, it took too
 * long, it could not be reached), keeps the phone's own approval apart from any computer's problem, and says when an
 * online computer is not providing Spaces to the phone.
 */

import type { ComputerRow } from "../../core/views";
import { OLD_COMPUTER_REFUSAL } from "../types";

export type ReachKind = "needs-update" | "timeout" | "unreachable";

/** One computer that did not answer, and why. */
export interface ReachProblem {
  computerId: string;
  computer: string;
  kind: ReachKind;
}

/** This phone is not enrolled yet: an enrolled computer has to approve it. `code` is the one-time code, when this phone has one. */
export interface WaitingApproval {
  code: string | null;
}

/** The relay's refusal for a phone that is pending or not enrolled (`NOT_ENROLLED`, "waiting for approval"). */
export const NOT_ENROLLED_REFUSAL = /not enrolled|waiting for approval|from an enrolled device/i;

const TIMEOUT = /timed? ?out|timeout|deadline|aborted|took too long/i;

const text = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Whether an error is the relay refusing because this phone is not approved yet. */
export function isNotEnrolled(e: unknown): boolean {
  return NOT_ENROLLED_REFUSAL.test(text(e));
}

/** What went wrong with one computer's answer; null when the error is this phone's own approval (that is not a computer's fault). */
export function reachKind(e: unknown): ReachKind | null {
  if (isNotEnrolled(e)) return null;
  if (e instanceof Error && e.name === "NeedsUpdateError") return "needs-update";
  if (OLD_COMPUTER_REFUSAL.test(text(e))) return "needs-update";
  const err = e as { name?: string; code?: unknown; status?: unknown } | null;
  // Connect `DeadlineExceeded` (4) and `Aborted` (10); the relay's 504.
  if (err && (err.code === 4 || err.code === 10 || err.status === 504 || err.name === "TimeoutError" || err.name === "AbortError")) return "timeout";
  return TIMEOUT.test(text(e)) ? "timeout" : "unreachable";
}

/** One line for a computer that did not answer. */
export function problemLine(p: ReachProblem): string {
  switch (p.kind) {
    case "needs-update":
      return `${p.computer} needs an update. Update Cua Spaces on it, then pull down to try again.`;
    case "timeout":
      return `${p.computer} took too long to answer. Pull down to try again.`;
    default:
      return `${p.computer} could not be reached. Check that it is on and online, then pull down to try again.`;
  }
}

/** The lines for all of them (one per computer, worst first: an update is something the person has to do). */
export function problemLines(ps: ReachProblem[]): string[] {
  const order: Record<ReachKind, number> = { "needs-update": 0, unreachable: 1, timeout: 2 };
  return [...ps].sort((a, b) => order[a.kind] - order[b.kind] || a.computer.localeCompare(b.computer)).map(problemLine);
}

/** The note under an online computer that is not providing Spaces to the phone (`cua.host.spaces` is off). */
export const NOT_PROVIDING_NOTE = "Not providing Spaces to your phone. In Cua Spaces turn on Provide Spaces, or run cua host setup --share spaces on it.";

/** Whether `c` is online, signed in and sharing, but does not provide Spaces. */
export function notProviding(c: ComputerRow): boolean {
  return !c.isPhone && !c.unconfirmed && c.online && c.state === "Online" && !c.hostsSpaces;
}

/** The pool's computers with the note a computer that is not providing Spaces gets. */
export function withProvidingNotes(computers: ComputerRow[]): ComputerRow[] {
  return computers.map((c) => (notProviding(c) ? { ...c, note: NOT_PROVIDING_NOTE } : c));
}

/** The exact command that approves this phone, run on a computer that is already signed in. */
export const approveCommand = (code: string | null) => `cua devices approve ${code ?? "<CODE>"}`;

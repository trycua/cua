// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Home's box starts on this computer once it's in the account (5 Nov 2026: "this computer already
 * counts; a new local computer is optional"). Making a Space first is never needed to ask for a job.
 * This file decides how this computer stands as a place to run, what the box starts on, and what the
 * one Set up action does when this computer isn't ready yet. Pure, so every state is tested without a host.
 */

import type { HostStatus } from "@/bridge/contracts/host";
import { t } from "@/i18n";

import { defaultWhere, hostSpaceId, type NewKind, type TaskWhere } from "./ask-first";

/**
 * How this computer stands as a place to run a job:
 * - `ready`: it's in the account and shares its desktop, so a job can start on it now;
 * - `needs-setup`: signed in, but it isn't set up yet, or doesn't share its desktop (its own Set up fixes that);
 * - `checking`: signed in, its host service not read yet;
 * - `signed-out`: no account, so the box starts on a new computer as before.
 */
export type ThisMacReadiness = "ready" | "needs-setup" | "checking" | "signed-out";

/** `status`: `host.status`; undefined while it's being read, null when it couldn't be. */
export function thisMacReadiness(signedIn: boolean, status: HostStatus | null | undefined): ThisMacReadiness {
  if (!signedIn) return "signed-out";
  if (status === undefined) return "checking";
  return hostSpaceId(status) ? "ready" : "needs-setup";
}

/** Where the box starts unless the person picks: this computer once signed in, else a new computer (`defaultWhere`). */
export function homeDefaultWhere(readiness: ThisMacReadiness, kinds: readonly NewKind[]): TaskWhere {
  return readiness === "signed-out" ? defaultWhere(kinds) : { kind: "this-mac" };
}

export type ThisMacSetupStep = "add" | "resume-sharing" | "share-desktop";

/**
 * What Set up does, in order: add this computer to the account if it isn't yet (`add`, the same
 * `addThisComputer` a sign-in runs), share again if sharing was stopped, then share its desktop so
 * agents can use it. Empty when there's nothing to do.
 */
export function thisMacSetupSteps(status: HostStatus | null | undefined): ThisMacSetupStep[] {
  if (!status?.configured) return ["add", "share-desktop"];
  const steps: ThisMacSetupStep[] = [];
  if (!status.sharing) steps.push("resume-sharing");
  if (status.shareDesktop === false) steps.push("share-desktop");
  return steps;
}

/** The chip's words when this computer isn't ready: `place` is "This Mac", "This PC" ... */
export const thisMacChipText = (place: string) => ({
  note: t("computers.this_mac.chip_note"),
  action: t("computers.this_mac.chip_action"),
  actionLabel: t("computers.this_mac.chip_action_label", { place }),
  busy: t("computers.this_mac.chip_busy"),
});

/**
 * This computer's host status once it can take a job: Set up sharing its desktop starts its sharing service
 * again, which is offline for a few seconds while it reconnects. A job sent in that moment failed with
 * "Couldn't reach that computer" (CUA-3974), so the start waits, up to `timeoutMs`, while the status says it is
 * set up but offline. Anything else (not set up, no status, online) is returned at once.
 */
export async function hostStatusWhenOnline(
  read: () => Promise<HostStatus | null>,
  { timeoutMs = 20_000, everyMs = 1_000, sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms)), now = () => Date.now() } = {},
): Promise<HostStatus | null> {
  const until = now() + timeoutMs;
  let status = await read();
  while (status && hostSpaceId(status) && status.online === false && now() < until) {
    await sleep(everyMs);
    status = await read();
  }
  return status;
}

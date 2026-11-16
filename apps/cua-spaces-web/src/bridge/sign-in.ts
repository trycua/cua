// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Sign-in you can see (the core's `sign_in`): the failed phase for a reason
 * or a caught error, and the sidebar's foot for every state. The words are
 * the core's; the fallbacks only keep the page usable before the wasm core
 * loads (tests, a dev build without it).
 */

import type { CoreClient } from "./core";
import type { SignInFailure, SignInFooter, SignInPhase } from "./contracts/host";

export type FailedPhase = Extract<SignInPhase, { kind: "failed" }>;

const FAILED = "Sign-in didn't finish";

/** The failed phase for `reason`, with the error's words for Details. */
export function signInFailedPhase(core: CoreClient, reason: SignInFailure, detail: string | null = null): FailedPhase {
  return core.tryCall<FailedPhase>("signIn.failed", { reason, detail }) ?? { kind: "failed", reason, message: `${FAILED}.`, detail };
}

/** The failed phase for an error the page or a host caught: its case
 * (`Timeout`, `Http`, ...: a UniFFI error's `tag`), if any, and its words. */
export function signInFailedFromError(core: CoreClient, error: unknown): FailedPhase {
  const message = error instanceof Error ? error.message : String(error);
  const tag = typeof (error as { tag?: unknown } | null)?.tag === "string" ? (error as { tag: string }).tag : null;
  return (
    core.tryCall<FailedPhase>("signIn.failedFromError", { variant: tag, message }) ?? {
      kind: "failed",
      reason: "unknown",
      message: `${FAILED}.`,
      detail: message || null,
    }
  );
}

/** What the sidebar's foot shows. */
export interface SignInFooterInput {
  identity: string | null;
  cloudConfigured: boolean;
  canSignIn: boolean;
  signIn: SignInPhase;
  hasUrl: boolean;
}

/** The sidebar's foot: signed in as, waiting with the code, Open again and
 * Cancel, or why it failed with Try again (and the toast to show once). */
export function signInFooter(core: CoreClient, input: SignInFooterInput): SignInFooter {
  return core.tryCall<SignInFooter>("signIn.footer", { input }) ?? fallbackFooter(input);
}

function fallbackFooter(input: SignInFooterInput): SignInFooter {
  const base: SignInFooter = {
    state: "signedOut",
    text: input.cloudConfigured ? "Signed in with an API key" : "Not signed in",
    identity: null,
    code: null,
    codeLabel: "Code",
    reopenLabel: null,
    cancelLabel: null,
    actionLabel: input.canSignIn ? "Sign in" : null,
    reason: null,
    detail: null,
    detailsLabel: "Details",
    toast: null,
  };
  if (input.identity) return { ...base, state: "signedIn", text: `Signed in as ${input.identity}`, identity: input.identity, actionLabel: null };
  const p = input.signIn;
  switch (p.kind) {
    case "starting":
      return { ...base, state: "starting", text: "Opening your browser…", cancelLabel: "Cancel", actionLabel: null };
    case "waiting":
      return {
        ...base,
        state: "waiting",
        text: p.opened === false ? "Open the sign-in page to continue." : "Waiting for your browser…",
        code: p.userCode ?? null,
        reopenLabel: input.hasUrl ? "Open again" : null,
        cancelLabel: "Cancel",
        actionLabel: null,
      };
    case "failed":
      return {
        ...base,
        state: "failed",
        text: p.message || `${FAILED}.`,
        reason: p.reason ?? "unknown",
        detail: p.detail ?? null,
        actionLabel: "Try again",
        toast: { title: FAILED, body: null },
      };
    default:
      return base;
  }
}

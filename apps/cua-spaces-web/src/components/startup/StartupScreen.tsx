// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { LoaderCircleIcon } from "lucide-react";
import { useState } from "react";

import type { StartupAction, StartupState } from "@/bridge";
import { CuaLogo } from "@/components/cua-logo";
import { Button } from "@/components/ui/button";

const LABELS: Record<StartupAction, string> = {
  allowAccess: "Allow access",
  tryAgain: "Try again",
  signInAgain: "Sign in again",
};

const CONFIRM_SIGN_IN_AGAIN =
  "Sign in again? This removes the saved sign-in from this Mac's keychain. Nothing else is deleted. You'll sign in again in your browser.";

/**
 * The whole window while the native app is still starting: the host's
 * title and words, a spinner while it waits, and its buttons. Sign in again
 * asks first, here, before it goes to the host.
 */
export function StartupScreen({ state, onAct }: { state: StartupState; onAct: (action: StartupAction) => Promise<void> }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const waiting = state.phase === "starting" || state.phase === "waitingForKeychain";

  const run = async (action: StartupAction) => {
    setBusy(true);
    try {
      await onAct(action);
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };
  const press = (action: StartupAction) => (action === "signInAgain" ? setConfirming(true) : void run(action));

  return (
    <div className="flex h-full flex-col bg-background" data-testid="startup-screen" data-phase={state.phase}>
      <div className="app-drag h-(--titlebar-height) shrink-0" />
      <main className="app-drag flex min-h-0 flex-1 flex-col items-center justify-center overflow-y-auto px-10 py-6">
        <div className="flex w-full max-w-[520px] flex-col items-center text-center" aria-busy={waiting || busy}>
          <CuaLogo className="mb-6 size-14" />
          <div role="status" aria-live="polite" className="flex flex-col items-center">
            <h1 className="flex items-center gap-2 text-[26px] font-semibold tracking-[-0.015em]">
              {waiting ? <LoaderCircleIcon className="size-5 animate-spin text-muted-foreground" aria-hidden /> : null}
              {state.title}
            </h1>
            {state.body ? <p className="mt-2 text-[14px] text-pretty text-muted-foreground">{state.body}</p> : null}
          </div>
          {confirming ? (
            <div className="mt-8 flex flex-col items-center gap-4" data-testid="startup-confirm">
              <p className="text-[13px] text-pretty">{CONFIRM_SIGN_IN_AGAIN}</p>
              <div className="flex gap-2">
                <Button variant="outline" disabled={busy} onClick={() => setConfirming(false)}>
                  Cancel
                </Button>
                <Button autoFocus disabled={busy} onClick={() => void run("signInAgain")}>
                  Sign in again
                </Button>
              </div>
            </div>
          ) : state.actions.length > 0 ? (
            <div className="mt-8 flex gap-2">
              {state.actions.map((action, i) => (
                <Button key={action} variant={i === 0 ? "default" : "outline"} autoFocus={i === 0} disabled={busy} onClick={() => press(action)}>
                  {LABELS[action]}
                </Button>
              ))}
            </div>
          ) : null}
        </div>
      </main>
    </div>
  );
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hostOs } from "@/lib/host-labels";

export const EXPLAINED_KEY = "cua.teleport.explained";

/** The explainer shows once, on a Mac, before the first run (macOS asks for permission during it). */
export function needsExplainer(): boolean {
  if (hostOs() !== "macos") return false;
  try {
    return localStorage.getItem(EXPLAINED_KEY) !== "1";
  } catch {
    return true;
  }
}

export function markExplained() {
  try {
    localStorage.setItem(EXPLAINED_KEY, "1");
  } catch {
    // Nothing to remember it in: it shows again next time.
  }
}

/** What teleport does and why the Mac asks, in plain words, before the prompt. */
export function TeleportExplainer() {
  return (
    <div className="space-y-4 px-5 pb-4 text-[13px] leading-relaxed" data-teleport-explainer>
      <p>
        <strong className="font-semibold">Teleport copies an app from your Mac to the Space, so you don't have to sign in again there.</strong> An agent working in the
        Space can then use the app as you.
      </p>
      <p>
        <strong className="font-semibold">Your Mac will ask for permission next.</strong> It may say an app wants to use confidential information stored in your Keychain.
        That is where your Mac keeps your saved sign-ins. Choose Allow so Cua can read the ones you picked.
      </p>
      <p className="text-muted-foreground">Only what you ticked on the last screen is copied. If you say no, the app still arrives, but signed out.</p>
    </div>
  );
}

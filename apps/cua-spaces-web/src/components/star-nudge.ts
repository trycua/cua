// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useContext } from "react";

import { BridgeContext } from "@/bridge/BridgeProvider";
import { toastManager } from "@/components/ui/toast";

/**
 * The "star us on GitHub" nudge: one Cua toast, once the person has sent
 * their 5th prompt to an agent (a message in a Space's thread, a message
 * to an agent, or a task from Home) and then stopped typing for a moment.
 * It stays until closed and never blocks anything. Its button opens the
 * repository in the browser; nothing records who starred.
 *
 * After "Star on GitHub" or the close button it never shows again (kept in
 * the page's storage, as the upsell prompts keep theirs). Left open when
 * the app quits, it may show once more at a later launch. Never on the
 * onboarding pages.
 *
 * Usage events (`cua_spaces_feature_used`, only while usage data is on):
 * `star_nudge_shown`, `star_nudge_click`, `star_nudge_dismiss`.
 */

export const STAR_URL = "https://github.com/trycua/cua";
export const PROMPTS_BEFORE_ASKING = 5;
/** How long without input before it shows, so it never lands mid-sentence. */
export const IDLE_MS = 30_000;

// Separate keys, so a count written later can never undo a close.
const SENT_KEY = "cua.starNudge.sent";
const CLOSED_KEY = "cua.starNudge.closed";
const ACTIVITY_EVENTS = ["pointerdown", "keydown", "wheel", "focus"] as const;

/** What the nudge needs from the app: usage events and the browser. */
export interface StarNudgeHost {
  track(feature: "star_nudge_shown" | "star_nudge_click" | "star_nudge_dismiss"): void;
  openExternal(url: string): Promise<void>;
}

let pending = false;

const onOnboarding = () => globalThis.location?.pathname.startsWith("/onboarding") ?? false;

function closed(): boolean {
  return globalThis.localStorage.getItem(CLOSED_KEY) === "true";
}

function markClosed(): void {
  try {
    globalThis.localStorage.setItem(CLOSED_KEY, "true");
  } catch {
    // Not kept: at worst it asks once more at a later launch.
  }
}

/** Resolves once there has been no input for `IDLE_MS`. */
function waitForIdle(): Promise<void> {
  return new Promise((resolve) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const restart = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        for (const type of ACTIVITY_EVENTS) window.removeEventListener(type, restart, true);
        resolve();
      }, IDLE_MS);
    };
    for (const type of ACTIVITY_EVENTS) window.addEventListener(type, restart, { capture: true, passive: true });
    restart();
  });
}

function show(host: StarNudgeHost): void {
  let clicked = false;
  const id = toastManager.add({
    title: "Enjoying Cua Spaces?",
    description: "Give us a star on GitHub.",
    type: "info",
    // Stays until the person closes it.
    timeout: 0,
    onClose: () => {
      markClosed();
      if (!clicked) host.track("star_nudge_dismiss");
    },
    data: {
      fromCua: true,
      starNudge: true,
      action: {
        label: "Star on GitHub",
        onClick: () => {
          clicked = true;
          host.track("star_nudge_click");
          void host.openExternal(STAR_URL).catch(() => undefined);
          toastManager.close(id);
        },
      },
    },
  });
  host.track("star_nudge_shown");
}

/**
 * Counts one prompt sent. At the 5th and after, once per launch until it
 * is closed, waits for a quiet moment and shows the nudge.
 */
export async function recordPromptSent(host: StarNudgeHost): Promise<void> {
  if (pending) return;
  try {
    if (closed()) return;
    const sent = (Number(globalThis.localStorage.getItem(SENT_KEY)) || 0) + 1;
    globalThis.localStorage.setItem(SENT_KEY, String(sent));
    if (sent < PROMPTS_BEFORE_ASKING) return;
  } catch {
    // Storage that can't be read or written can't keep a close either: never ask.
    return;
  }
  pending = true;
  await waitForIdle();
  try {
    // Closed in another window meanwhile, or the person is in onboarding:
    // in onboarding, try again at the next prompt.
    if (closed()) return;
    if (onOnboarding()) {
      pending = false;
      return;
    }
  } catch {
    return;
  }
  show(host);
}

/** `recordPromptSent` bound to this app's usage data and browser. Without a bridge it does nothing. */
export function useStarNudge(): () => void {
  const ready = useContext(BridgeContext)?.ready;
  return useCallback(() => {
    if (!ready) return;
    void recordPromptSent({
      track: (feature) => void ready.then((s) => s.telemetry.track([{ type: "feature", feature }])),
      openExternal: (url) => ready.then((s) => s.openExternal(url)),
    });
  }, [ready]);
}

/** Forget the count, a close and this launch's nudge (tests). */
export function resetStarNudge(): void {
  pending = false;
  try {
    globalThis.localStorage.removeItem(SENT_KEY);
    globalThis.localStorage.removeItem(CLOSED_KEY);
  } catch {
    // Nothing kept.
  }
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Msg } from "../bridge/contracts/teleport";

/**
 * English for the core's message codes (`message::Msg`). This site is
 * English only; the app's catalog is `apps/cua-spaces/src/i18n/locales/en`,
 * and `messages.test.ts` fails if the two drift.
 */
export const MESSAGES: Record<string, string> = {
  "teleport.sensitive.sign_ins.label": "Keep me signed in",
  "teleport.sensitive.sign_ins.detail": "Sends the session cookies.",
  "teleport.sensitive.passwords.label": "Saved passwords",
  "teleport.sensitive.passwords.detail": "Sends the passwords saved in the app.",
  "teleport.sensitive.history.label": "Browsing history",
  "teleport.sensitive.history.detail":
    "Sends the history of pages visited, and the bookmarks where the browser keeps them together.",
  "spaces.power.suspend": "Suspend",
  "spaces.power.resume": "Resume",
  "spaces.power.turn_off": "Turn off",
  "spaces.power.turn_on": "Turn on",
  "spaces.status.failed": "Failed",
  "spaces.status.cancelling": "Cancelling…",
  "spaces.status.this_mac": "This Mac",
  "spaces.status.running": "Running",
  "spaces.status.local": "Local",
  "spaces.status.approval": "Needs approval",
  "spaces.status.suspended": "Suspended",
  "spaces.status.starting": "Starting",
  "spaces.status.deleting": "Deleting…",
  "spaces.status.stopped": "Stopped",
  "spaces.status.stopped_reason": "Stopped: {reason}",
  "spaces.status.suspending": "Suspending…",
  "spaces.status.resuming": "Resuming…",
  "spaces.status.turning_off": "Turning off…",
  "spaces.status.turning_on": "Turning on…",
};

/** A core message as text: its English with the variables filled in. */
export function messageText(msg: Msg): string {
  const text = MESSAGES[msg.code] ?? msg.code;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => msg.vars[name] ?? whole);
}

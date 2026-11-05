// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * When the viewer may move the clipboard between this computer and the
 * Space. Pure on purpose: the viewer passes the facts and moves data only on
 * "allow". The native viewer applies the same rule (`clipboard_gate.rs`).
 *
 * - "view" (Secure Spaces): nothing moves while the tab is not focused and
 *   shown. Paste sends the clipboard once, copy brings the Space's once, and
 *   a change the Space makes on its own needs a click.
 * - "ask": every transfer needs a click.
 * - "on" (the default without the parameter): the old behaviour, the
 *   clipboard follows both ways.
 * - "off": nothing moves.
 */

export type ClipboardMode = "view" | "ask" | "on" | "off";
export type ClipboardDirection = "to-guest" | "to-host";
/** What started a transfer: the user pasted, the user copied, or the Space changed its clipboard. */
export type ClipboardOrigin = "paste" | "copy" | "guest";
export type ClipboardDecision = "allow" | "prompt" | "deny";

/**
 * The `clipboard` URL parameter. Without it the clipboard follows both ways,
 * as it always did ("on"): the gate applies when Cua asks for it
 * (`clipboard=view` or `ask`, which Secure Spaces puts in the viewer link).
 * A value that is not known is the safe one.
 */
export function parseClipboardMode(param: string | null | undefined): ClipboardMode {
  if (param === null || param === undefined || param.trim() === "") return "on";
  switch (param.trim().toLowerCase()) {
    case "off":
    case "0":
      return "off";
    case "on":
    case "1":
      return "on";
    case "ask":
      return "ask";
    case "view":
    case "viewing":
    default:
      return "view";
  }
}

export interface ClipboardFacts {
  mode: ClipboardMode;
  /** The tab is focused and shown. */
  viewing: boolean;
  direction: ClipboardDirection;
  origin: ClipboardOrigin;
  /** The user chose "Always while I'm viewing" and has not left since. */
  always: boolean;
}

export function decideClipboard(f: ClipboardFacts): ClipboardDecision {
  if (f.mode === "off") return "deny";
  if (f.mode === "on") return "allow";
  if (!f.viewing) return "deny";
  const byUser =
    (f.direction === "to-guest" && f.origin === "paste") || (f.direction === "to-host" && f.origin === "copy");
  if (!byUser && !(f.direction === "to-host" && f.origin === "guest")) return "deny";
  if (f.mode === "view" && (byUser || f.always)) return "allow";
  return "prompt";
}

/** Whether the prompt may offer "Always while I'm viewing". */
export function offersAlways(mode: ClipboardMode): boolean {
  return mode === "view";
}

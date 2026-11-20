// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { InteractiveInputEvent } from "./mediaWire";

const COMMAND_CODES = new Set(["MetaLeft", "MetaRight"]);

type GuestKey = "meta" | "control";

/**
 * The ⌘ keys a viewer holds and the guest keys they hold down. On a guest
 * that takes ⌘ as Control, the right ⌘ held without the left one is the
 * guest's Super (`meta`) and any other ⌘ is Control; elsewhere ⌘ is `meta`.
 * After every key event the guest's keys follow the ⌘ keys held, so a ⌘
 * joining or leaving swaps Super and Control on the guest.
 */
export class CommandKeys {
  private readonly held = new Set<string>();
  // Set once focus left: a ⌘ may have been released elsewhere, so the next
  // ⌘ pressed replaces what `held` holds.
  private unsure = false;
  private readonly down = new Set<GuestKey>();

  constructor(private readonly metaAsControl: () => boolean) {}

  static isCommand(code: string): boolean {
    return COMMAND_CODES.has(code);
  }

  /** Whether ⌘ is the guest's Super now. */
  get isSuper(): boolean {
    return this.metaAsControl() && this.held.has("MetaRight") && !this.held.has("MetaLeft");
  }

  /** A key went down or up: the guest key events that bring the guest's ⌘
   * keys in line, to send before the key's own event. */
  key(event: { code: string; metaKey: boolean }, state: "down" | "up"): InteractiveInputEvent[] {
    const command = COMMAND_CODES.has(event.code);
    if (!event.metaKey || (state === "down" && this.unsure && command)) {
      this.held.clear();
      this.unsure = false;
    }
    if (command && state === "down") this.held.add(event.code);
    if (command && state === "up") this.held.delete(event.code);
    const want = new Set<GuestKey>();
    if (this.held.size > 0) want.add(this.metaAsControl() && !this.isSuper ? "control" : "meta");
    const out: InteractiveInputEvent[] = [];
    for (const key of this.down) {
      if (want.has(key)) continue;
      this.down.delete(key);
      out.push(guestKey(key, "up"));
    }
    for (const key of want) {
      if (this.down.has(key)) continue;
      this.down.add(key);
      out.push(guestKey(key, "down"));
    }
    return out;
  }

  /** Focus left: release the guest's ⌘ keys. Which ⌘ is held is kept for
   * the next key, since one held through the change is still down. */
  release(): InteractiveInputEvent[] {
    const out = [...this.down].map((key) => guestKey(key, "up"));
    this.down.clear();
    this.unsure = this.held.size > 0;
    return out;
  }
}

function guestKey(key: GuestKey, state: "down" | "up"): InteractiveInputEvent {
  return { kind: "key", key, state, modifiers: [], repeat: false };
}

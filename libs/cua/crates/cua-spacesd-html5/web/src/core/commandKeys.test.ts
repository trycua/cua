// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { CommandKeys } from "./commandKeys";

const LEFT = "MetaLeft";
const RIGHT = "MetaRight";

/** Feeds key events and returns the guest keys each one sent, as "key state". */
function drive(metaAsControl: boolean) {
  const keys = new CommandKeys(() => metaAsControl);
  const sent = (events: ReturnType<CommandKeys["key"]>) => events.map((e) => (e.kind === "key" ? `${e.key} ${e.state}` : e.kind));
  return {
    keys,
    down: (code: string, metaKey = true) => sent(keys.key({ code, metaKey }, "down")),
    up: (code: string, metaKey: boolean) => sent(keys.key({ code, metaKey }, "up")),
    release: () => sent(keys.release()),
  };
}

describe("command keys for a guest that takes ⌘ as Control", () => {
  it("holds Super for the right ⌘ alone and Control for any other ⌘", () => {
    const k = drive(true);
    expect(k.down(RIGHT)).toEqual(["meta down"]);
    expect(k.keys.isSuper).toBe(true);
    // The left ⌘ joining swaps Super for Control on the guest.
    expect(k.down(LEFT)).toEqual(["meta up", "control down"]);
    expect(k.keys.isSuper).toBe(false);
    // Released in either order, the guest ends with nothing held.
    expect(k.up(RIGHT, true)).toEqual([]);
    expect(k.up(LEFT, false)).toEqual(["control up"]);
    expect(k.down(LEFT)).toEqual(["control down"]);
    expect(k.down(RIGHT)).toEqual([]);
    // The left ⌘ leaving makes the right one Super again.
    expect(k.up(LEFT, true)).toEqual(["control up", "meta down"]);
    expect(k.up(RIGHT, false)).toEqual(["meta up"]);
  });

  it("releases the guest's keys on focus loss and keeps the ⌘ held for the next key", () => {
    const k = drive(true);
    k.down(RIGHT);
    expect(k.release()).toEqual(["meta up"]);
    expect(k.release()).toEqual([]);
    // Still held on return: the next chord presses Super again.
    expect(k.down("KeyW")).toEqual(["meta down"]);
    expect(k.keys.isSuper).toBe(true);
    // Its late key-up releases it once.
    expect(k.up(RIGHT, false)).toEqual(["meta up"]);
  });

  it("replaces a ⌘ that may have been released while unfocused with the next one pressed", () => {
    const k = drive(true);
    k.down(LEFT);
    k.release();
    expect(k.down(RIGHT)).toEqual(["meta down"]);
    expect(k.keys.isSuper).toBe(true);
    // A key with no ⌘ held clears the rest.
    expect(k.down("KeyA", false)).toEqual(["meta up"]);
    expect(k.keys.isSuper).toBe(false);
  });
});

describe("command keys for other guests", () => {
  it("holds one ⌘ on the guest while either is down", () => {
    const k = drive(false);
    expect(k.down(RIGHT)).toEqual(["meta down"]);
    expect(k.keys.isSuper).toBe(false);
    expect(k.down(LEFT)).toEqual([]);
    expect(k.up(RIGHT, true)).toEqual([]);
    expect(k.up(LEFT, false)).toEqual(["meta up"]);
  });
});

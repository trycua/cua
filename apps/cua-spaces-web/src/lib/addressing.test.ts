// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { defaultAddressee, EMPTY_ADDRESS, pinAgent, resetAddressing, resolveAddressing, setAddress, unpinAgent, useAddressing, type ResolveInput } from "./addressing";

const base = (over: Partial<ResolveInput> = {}): ResolveInput => ({
  text: "run the tests",
  agents: ["claude", "hermes"],
  agentHandles: new Set(["claude", "hermes", "codex"]),
  known: new Set(["claude", "hermes", "codex", "grace", "everyone"]),
  canEveryone: true,
  state: EMPTY_ADDRESS,
  ...over,
});

describe("who a message goes to", () => {
  it("sends a plain message as it is when nobody is the default", () => {
    const r = resolveAddressing(base());
    expect(r.text).toBe("run the tests");
    expect(r.addressee).toEqual({ handles: [], source: "none" });
    expect(r.auto).toBe(false);
  });

  it("needs no @ with exactly one agent in the Space", () => {
    const r = resolveAddressing(base({ agents: ["claude"] }));
    expect(r.text).toBe("@claude run the tests");
    expect(r.addressee).toEqual({ handles: ["claude"], source: "only" });
    expect(r.auto).toBe(true);
  });

  it("defaults to the last addressed agent when several are here", () => {
    const first = resolveAddressing(base({ text: "@hermes draft the note" }));
    expect(first.text).toBe("@hermes draft the note");
    expect(first.addressee).toEqual({ handles: ["hermes"], source: "typed" });
    expect(first.next.last).toBe("hermes");
    const next = resolveAddressing(base({ state: first.next, text: "and shorter" }));
    // The typed mention pinned it: the pin is what carries on.
    expect(next.text).toBe("@hermes and shorter");
    expect(next.addressee.source).toBe("pinned");
    // Without a pin (the chip was removed), the last addressed one is the default.
    const quiet = resolveAddressing(base({ state: { ...first.next, pinned: [] }, text: "and shorter" }));
    expect(quiet.text).toBe("@hermes and shorter");
    expect(quiet.addressee.source).toBe("last");
  });

  it("lets an @ you typed win and become the pin", () => {
    const state = { ...EMPTY_ADDRESS, pinned: ["claude"], last: "claude" };
    const r = resolveAddressing(base({ state, text: "@hermes take this one" }));
    expect(r.text).toBe("@hermes take this one");
    expect(r.next.pinned).toEqual(["hermes"]);
    expect(r.next.last).toBe("hermes");
  });

  it("ends the pin when you @ a person, and adds nothing", () => {
    const state = { ...EMPTY_ADDRESS, pinned: ["claude"], last: "claude" };
    const r = resolveAddressing(base({ state, text: "@grace can you look?" }));
    expect(r.text).toBe("@grace can you look?");
    expect(r.addressee.handles).toEqual([]);
    expect(r.next.pinned).toEqual([]);
  });

  it("counts a reply to an agent's message as addressing it", () => {
    const r = resolveAddressing(base({ text: "why that one?", replyTo: "Hermes" }));
    expect(r.text).toBe("@hermes why that one?");
    expect(r.addressee).toEqual({ handles: ["hermes"], source: "reply" });
    expect(r.next.pinned).toEqual(["hermes"]);
  });

  it("names @everyone only when it is allowed", () => {
    expect(resolveAddressing(base({ text: "@everyone status?" })).addressee).toEqual({ handles: ["everyone"], source: "typed" });
    const off = resolveAddressing(base({ text: "@everyone status?", canEveryone: false }));
    expect(off.addressee.handles).toEqual([]);
    expect(off.text).toBe("@everyone status?");
  });

  it("keeps a pinned @everyone only while it is allowed and agents are here", () => {
    const state = { ...EMPTY_ADDRESS, pinned: ["everyone"] };
    expect(defaultAddressee({ agents: ["claude", "hermes"], canEveryone: true, state }).handles).toEqual(["everyone"]);
    expect(defaultAddressee({ agents: ["claude", "hermes"], canEveryone: false, state }).source).toBe("none");
  });

  it("drops a pinned or last agent that left the Space", () => {
    const state = { ...EMPTY_ADDRESS, pinned: ["codex"], last: "codex" };
    expect(defaultAddressee({ agents: ["claude", "hermes"], canEveryone: true, state })).toEqual({ handles: [], source: "none" });
  });

  it("stays plain after the chip's X until someone is addressed again", () => {
    const state = { ...EMPTY_ADDRESS, off: true };
    expect(resolveAddressing(base({ agents: ["claude"], state })).text).toBe("run the tests");
    expect(resolveAddressing(base({ agents: ["claude"], state, text: "@claude hi" })).next.off).toBe(false);
  });
});

describe("what is kept per Space", () => {
  beforeEach(resetAddressing);

  it("pins on a Reply, removes with the chip's X, and keeps each Space apart", () => {
    pinAgent("a", "Claude");
    pinAgent("b", "hermes");
    expect(useAddressing.getState().bySpace.a).toMatchObject({ pinned: ["claude"], last: "claude", off: false });
    expect(useAddressing.getState().bySpace.b?.pinned).toEqual(["hermes"]);
    unpinAgent("a", "claude", "pinned");
    // With nothing left pinned, a single-agent default does not come straight back.
    expect(useAddressing.getState().bySpace.a).toMatchObject({ pinned: [], off: true });
    unpinAgent("b", "hermes", "only");
    expect(useAddressing.getState().bySpace.b).toMatchObject({ pinned: [], off: true });
  });

  it("is written to this machine so a reload keeps it", () => {
    setAddress("a", { pinned: ["claude"], last: "claude", off: false, told: true });
    expect(JSON.parse(localStorage.getItem("cua.thread.addressing")!)).toEqual({ a: { pinned: ["claude"], last: "claude", off: false, told: true } });
  });
});

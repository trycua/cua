// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { AgentSetupRow } from "@/bridge";
import { demoAgentSetup } from "@/bridge/adapters/demo-agents";
import { EMPTY_STATE, failureText, getItUrl, missingApps, planSync, readState, shownApps, STATE_KEY, withSwitch, writeState } from "./connected-apps";

const rows = (...over: Partial<AgentSetupRow>[]): AgentSetupRow[] =>
  over.map((o) => ({ ...demoAgentSetup()[0]!, ...o }) as AgentSetupRow);

describe("planSync", () => {
  it("only records what is installed on the first read, even with consent", () => {
    const r = rows({ agent: "claude-code", installed: true, configured: false }, { agent: "cursor", installed: true, configured: false });
    const plan = planSync(r, { ...EMPTY_STATE, auto: true });
    expect(plan.setUp).toEqual([]);
    expect(plan.state.known).toEqual(["claude-code", "cursor"]);
  });

  it("sets up a newly installed app when the person said yes", () => {
    const r = rows({ agent: "claude-code", installed: true, configured: true }, { agent: "cursor", installed: true, configured: false });
    const plan = planSync(r, { known: ["claude-code"], off: [], auto: true });
    expect(plan.setUp).toEqual(["cursor"]);
    expect(plan.state.known).toEqual(["claude-code", "cursor"]);
  });

  it("sets up nothing without a yes, and says the app is known so it is not new again", () => {
    const r = rows({ agent: "cursor", installed: true, configured: false });
    for (const auto of [null, false]) {
      const plan = planSync(r, { known: [], off: [], auto });
      expect(plan.setUp).toEqual([]);
      expect(plan.state.known).toEqual(["cursor"]);
    }
  });

  it("never touches an app that was turned off, or one that is already set up", () => {
    const r = rows({ agent: "cursor", installed: true, configured: false }, { agent: "hermes", installed: true, configured: true });
    expect(planSync(r, { known: [], off: ["cursor"], auto: true }).setUp).toEqual([]);
  });

  it("ignores apps that are not installed", () => {
    const r = rows({ agent: "kiro", installed: false, configured: false });
    expect(planSync(r, { known: [], off: [], auto: true })).toEqual({ state: { known: [], off: [], auto: true }, setUp: [] });
  });
});

describe("withSwitch", () => {
  it("remembers off and forgets it on on", () => {
    const off = withSwitch(EMPTY_STATE, "cursor", false);
    expect(off.off).toEqual(["cursor"]);
    expect(withSwitch(off, "cursor", false).off).toEqual(["cursor"]);
    expect(withSwitch(off, "cursor", true).off).toEqual([]);
  });
});

describe("the saved state", () => {
  const store = () => {
    const m = new Map<string, string>();
    return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), m };
  };
  it("round-trips and survives a damaged value", () => {
    const s = store();
    writeState(s, { known: ["a"], off: ["b"], auto: true });
    expect(readState(s)).toEqual({ known: ["a"], off: ["b"], auto: true });
    s.m.set(STATE_KEY, "{nope");
    expect(readState(s)).toEqual(EMPTY_STATE);
    s.m.set(STATE_KEY, JSON.stringify({ known: "x", off: [1, "c"], auto: "yes" }));
    expect(readState(s)).toEqual({ known: null, off: ["c"], auto: null });
    expect(readState(null)).toEqual(EMPTY_STATE);
  });
});

describe("what is listed", () => {
  const r = rows({ agent: "a", installed: true }, { agent: "b", installed: false, configured: true }, { agent: "c", installed: false, configured: false });
  it("lists installed apps and ones still set up; the rest are missing", () => {
    expect(shownApps(r).map((x) => x.agent)).toEqual(["a", "b"]);
    expect(missingApps(r).map((x) => x.agent)).toEqual(["c"]);
  });
  it("links only the apps we are sure of", () => {
    expect(getItUrl("cursor")).toMatch(/^https:\/\/cursor\.com/);
    expect(getItUrl("pi")).toBeNull();
  });
});

describe("failureText", () => {
  it("says a read-only file plainly and never shows an os error", () => {
    expect(failureText("Hermes", "Permission denied (os error 13)")).toBe("Couldn't set up Hermes. Cua can't change this file because it is read-only or belongs to another user.");
    expect(failureText("Hermes", "boom")).toBe("Couldn't set up Hermes. Cua said: boom.");
    expect(failureText("Hermes", "Not set up")).toBe("Couldn't set up Hermes.");
    expect(failureText("Hermes", "")).toBe("Couldn't set up Hermes.");
  });
});

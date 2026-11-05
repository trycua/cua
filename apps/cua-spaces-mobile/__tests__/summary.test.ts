// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { listView, signInFailure } from "../src/core/views";
import { createDemoBackend } from "../src/data/demo/backend";
import { spaceTone } from "../src/data/summary";
import { installFixtureCore, uninstallCore } from "./helpers/core";

beforeAll(installFixtureCore);
afterAll(uninstallCore);

describe("Home's words come from the app core", () => {
  it("says Loading and Reaching in the core's words", () => {
    expect(listView({ loaded: false, incomplete: false, reaching: 0, count: 0 })?.reachingLine).toBe("Loading Spaces…");
    expect(listView({ loaded: true, incomplete: true, reaching: 2, count: 3 })?.reachingLine).toBe("Reaching 2 machines…");
    expect(listView({ loaded: true, incomplete: false, reaching: 0, count: 0 })?.empty).toBe(true);
  });

  it("says why a sign-in did not finish in the core's words", () => {
    expect(signInFailure("access_denied")?.message).toMatch(/declined/i);
  });
});

describe("one status per Space", () => {
  it("takes the most urgent of its agents, and an offline machine shows as stopped", () => {
    const rows = [{ tone: "working" }, { tone: "you" }, { tone: "asleep" }] as never;
    expect(spaceTone(rows, true)).toBe("you");
    expect(spaceTone(rows, false)).toBe("stopped");
    expect(spaceTone([], true)).toBe("idle");
  });

  it("the sample list orders Needs you first and keeps one status per row", async () => {
    installFixtureCore();
    const b = createDemoBackend({ now: () => 1_800_000_000_000 });
    const list = await b.listSpaces();
    expect(list.spaces[0]).toMatchObject({ name: "checkout-api", statusWord: "Needs you" });
    expect(list.spaces.map((s) => s.statusWord)).toEqual(expect.arrayContaining(["Needs you", "Running", "Asleep"]));
    // The agents list is the core's: its rows and lines match the fixture the Rust test pins.
    expect(list.agents[0]!.line).toMatch(/Waiting for you in checkout-api/);
  });
});

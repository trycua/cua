// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import { ciInitial, ciReduce, ciView } from "../ci-runners";
import { replayFlow } from "../parity";
import { testCore, wasmBuilt } from "./testCore";

describe.skipIf(!wasmBuilt)("CI Runners (wasm core)", () => {
  it("replays the parity flow through the bridge to the golden", async () => {
    const core = await testCore();
    const r = replayFlow(core, "ci-runners");
    expect(r.transcript).toEqual(r.golden);
    expect(r.bridged).toEqual(expect.arrayContaining(["ciRunners.reduce", "ciRunners.view"]));
  });

  it("draws the demo host's overview, and a tab and an open row change the page", async () => {
    const core = await testCore();
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 1 });
    const overview = await demo.call("ci.overview", { live: true });
    let state = ciInitial(core)!;
    const v = ciView(core, overview, state)!;
    expect(v.title).toBe("CI Runners");
    expect(v.setup).toBeNull();
    expect(v.jobs.tabs.map((t) => `${t.label} ${t.count}`)).toEqual(["All 7", "Running 1", "Failed 2"]);
    state = ciReduce(core, state, { type: "tab", tab: "failed" })!;
    state = ciReduce(core, state, { type: "expand", key: "9910/295" })!;
    const failed = ciView(core, overview, state)!;
    expect(failed.jobs.rows.map((r) => r.title)).toEqual(["lint", "ui tests"]);
    expect(failed.jobs.rows[0]!.detail?.openUrl).toBe("https://github.com/acme/api/actions/runs/9910/job/295");
    demo.dispose?.();
  });
});

describe("the demo host's CI Runners", () => {
  it("connects GitHub in steps: sign-in check, a page to open, then connected", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 1, ci: "fresh" });
    expect((await demo.call("ci.overview", {})).github.connected).toBe(false);
    expect(await demo.call("ci.connect", {})).toMatchObject({ state: "waiting", url: expect.stringContaining("github.com") });
    expect(await demo.call("ci.connect", {})).toMatchObject({ state: "waiting" });
    expect(await demo.call("ci.connect", {})).toMatchObject({ state: "connected" });
    const o = await demo.call("ci.overview", { live: true });
    expect(o.github).toMatchObject({ connected: true, login: "acme" });
    expect(o.setup.map((s) => s.state)).toEqual(["done", "done", "todo", "todo"]);
    demo.dispose?.();
  });

  it("refuses a token GitHub would refuse, and connects with one it would take", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 1, ci: "fresh" });
    expect(await demo.call("ci.connectToken", { org: "acme", token: "nope" })).toMatchObject({ state: "failed" });
    expect(await demo.call("ci.connectToken", { org: "acme", token: "github_pat_x" })).toMatchObject({ state: "connected" });
    demo.dispose?.();
  });

  it("switches a repository and the goal", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 1 });
    await demo.call("ci.repositories", { allow: ["acme/website"], remove: ["acme/docs"] });
    const o = await demo.call("ci.overview", {});
    expect(o.repositories.all?.filter((r) => r.allowed).map((r) => r.name)).toEqual(["acme/app", "acme/api", "acme/website"]);
    expect(await demo.call("ci.goal", { target: "aws", goal: "fastest" })).toEqual({ goal: "fastest", applied: true });
    expect((await demo.call("ci.overview", {})).targets[0]!.goal).toBe("fastest");
    demo.dispose?.();
  });
});

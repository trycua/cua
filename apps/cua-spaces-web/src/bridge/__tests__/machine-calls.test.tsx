// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import type { SpaceRow } from "../contracts/spaces";
import type { CoreClient } from "../core";
import { BridgeProvider, useAgents, useSpaceDetail, useSpaces, type AgentsHook, type SpacesHook } from "../index";
import { noCore, testCore, wasmBuilt } from "./testCore";

afterEach(cleanup);

/** One of your machines through the relay, as the SwiftUI host lists it once
 * it connected: `features` are what it reported supported. */
const machine = (id: string, name: string, features: string[]): SpaceRow => ({
  id: `relay:${id}`,
  name,
  provider: "relay",
  spacesdVersion: "0.4.1",
  features,
  os: "macos",
  reachable: true,
});

const SPARE = machine("spare01", "Spare Mac", ["host_spaces", "files"]);
const DESK = machine("desk01", "Desk Mac", ["desktop_stream", "window_stream", "host_spaces", "files"]);

/** The demo host with two more machines listed, and every call it gets
 * recorded. */
function withMachines(calls: [string, unknown][]) {
  const base = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: (async (op: string, args: unknown) => {
      calls.push([op, args]);
      const result = await (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
      if (op === "spaces.list") return [...(result as SpaceRow[]), SPARE, DESK];
      if (op === "agents.runs") return [];
      return result;
    }) as DataAdapter["call"],
  });
  return adapter;
}

const asked = (calls: [string, unknown][], op: string) => calls.filter(([o]) => o === op).map(([, a]) => (a as { spaceId: string }).spaceId);

function mountAgents(core: CoreClient, calls: [string, unknown][]) {
  const got: { agents?: AgentsHook } = {};
  function Probe() {
    got.agents = useAgents();
    return null;
  }
  render(
    <BridgeProvider adapter={withMachines(calls)} core={core}>
      <Probe />
    </BridgeProvider>,
  );
  return got;
}

describe.skipIf(!wasmBuilt)("a machine that does not share its desktop (wasm core)", () => {
  it("is not asked for agent runs, which start a process on it; one that shares is", async () => {
    const calls: [string, unknown][] = [];
    const got = mountAgents(await testCore(), calls);
    await waitFor(() => expect(got.agents?.data).toBeDefined());
    const runs = asked(calls, "agents.runs");
    expect(runs).toContain("relay:desk01");
    expect(runs).toContain("local:design-review");
    expect(runs).not.toContain("relay:spare01");
  });

  it("is not watched for usage and windows, which its desktop would have to answer", async () => {
    const calls: [string, unknown][] = [];
    const core = await testCore();
    const got: { spaces?: SpacesHook } = {};
    function Probe({ id }: { id: string | null }) {
      got.spaces = useSpaces();
      const space = got.spaces.data?.find((s) => s.id === id);
      useSpaceDetail(space, null);
      return null;
    }
    const ui = (id: string | null) => (
      <BridgeProvider adapter={withMachines(calls)} core={core}>
        <Probe id={id} />
      </BridgeProvider>
    );
    const view = render(ui(null));
    await waitFor(() => expect(got.spaces?.data?.some((s) => s.id === "relay:spare01")).toBe(true));
    for (const id of ["relay:spare01", "relay:desk01"]) view.rerender(ui(id));
    await waitFor(() => expect(asked(calls, "spaces.windows")).toContain("relay:desk01"));
    expect(asked(calls, "spaces.usage")).toContain("relay:desk01");
    expect([...asked(calls, "spaces.windows"), ...asked(calls, "spaces.usage")]).not.toContain("relay:spare01");
  });

  it("is asked as before when the core cannot say", async () => {
    const calls: [string, unknown][] = [];
    const got = mountAgents(noCore, calls);
    await waitFor(() => expect(got.agents?.data).toBeDefined());
    expect(asked(calls, "agents.runs")).toContain("relay:spare01");
  });
});

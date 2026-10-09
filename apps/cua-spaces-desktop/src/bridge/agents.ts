// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Agents page and Settings → AI agents (`agents.*`): the persistent
// agents, a Space's runs and their events, and the coding agents on this
// machine (the SwiftUI host's `agents`, `PersistentModel`, `AgentRunsModel`).
import type { AppModel } from "../model/app-model";
import type { BridgeContext } from "./context";
import { count, string, strings } from "./args";
import { Failure, type Handlers } from "./host";
import { encode } from "./value";

/** What `agents.changed` is told by: the agents, the coding agents' rows and the changes running. */
export function agentsSignature(model: AppModel): unknown {
  const a = model.agents;
  return [a.persistent.input.agents ?? null, encode(a.coding.rows), a.coding.pending];
}

export function agentsMethods({ model }: BridgeContext): Handlers {
  // Read at call time: the registry is built before anything runs.
  const needSetup = () => {
    if (!model.agents.coding.setup) throw Failure.unsupported("Agent setup is not available in this build");
  };
  const pauseOrResume = (pause: boolean) => async (args: Record<string, unknown>) => {
    const name = string(args, "name");
    const persistent = model.agents.persistent;
    if (persistent.state.busy) throw Failure.failed("Another change is still running");
    const action = pause ? new model.native.AppAgentsAction.Pause({ name }) : new model.native.AppAgentsAction.Resume({ name });
    await persistent.send(action);
    if (persistent.state.error !== undefined) throw Failure.failed(persistent.state.error);
    return null;
  };
  return {
    "agents.list": async () => {
      const persistent = model.agents.persistent;
      if (!persistent.canCallTools) throw Failure.unsupported("Agents need the cua daemon");
      return persistent.list();
    },
    "agents.runs": async (args) => {
      const runs = model.agents.runs(string(args, "spaceId"));
      await runs.refresh();
      switch (runs.load.kind) {
        case "ready":
          return encode(runs.load.rows);
        case "failed":
          throw Failure.failed(runs.load.message);
        case "loading":
          return [];
      }
    },
    "agents.events": async (args) => {
      const persistent = model.agents.persistent;
      if (!persistent.canCallTools) throw Failure.unsupported("A run's conversation needs the cua daemon");
      const space = string(args, "spaceId");
      const runId = string(args, "runId");
      return persistent.runEvents(space, runId, count(args, "cursor") ?? 0, count(args, "max"));
    },
    "agents.pause": pauseOrResume(true),
    "agents.resume": pauseOrResume(false),
    "agents.setup": async () => {
      needSetup();
      const coding = model.agents.coding;
      await coding.reload();
      return encode(coding.rows ?? []);
    },
    "agents.configure": async (args) => {
      needSetup();
      const coding = model.agents.coding;
      if (args.agents === undefined || args.agents === null) await coding.configureAll();
      else await coding.act(strings(args, "agents"), false);
      return encode(coding.rows ?? []);
    },
    "agents.setupDriver": async (args) => {
      needSetup();
      return encode(await model.agents.coding.setup!.setUpCuaDriver(strings(args, "agents")));
    },
  };
}

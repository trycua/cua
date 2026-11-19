// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { streamMethods } from "../src/bridge/stream";
import type { BridgeContext } from "../src/bridge/context";

function methods(recorded: string[], recordFeature = (name: string) => void recorded.push(name)) {
  const ctx = {
    supervisor: null,
    env: { CUA_SPACES_TEST_STREAM_URL: "ws://127.0.0.1:1/{id}/{tier}" },
    model: { recordFeature },
  } as unknown as BridgeContext;
  const handler = streamMethods(ctx)["spaces.openStream"]!;
  return (args: Record<string, unknown>) => Promise.resolve(handler(args, {} as never));
}

describe("spaces.openStream usage events", () => {
  it("records space_open_viewer when a full viewer opens, not for grid tiles or reconnects", async () => {
    const recorded: string[] = [];
    const open = methods(recorded);
    await open({ spaceId: "local:a", tier: "tile" });
    expect(recorded).toEqual([]);
    await open({ spaceId: "local:a", tier: "full" });
    expect(recorded).toEqual(["space_open_viewer"]);
    await open({ spaceId: "local:a", tier: "full", epoch: 2 });
    expect(recorded).toEqual(["space_open_viewer"]);
  });

  it("never fails a stream because telemetry threw", async () => {
    const open = methods([], () => {
      throw new Error("telemetry down");
    });
    await expect(open({ spaceId: "local:a", tier: "full" })).resolves.toMatchObject({ wsUrl: "ws://127.0.0.1:1/local%3Aa/full" });
  });
});

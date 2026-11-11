// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Home's line under the box: "Starting agent" is for an agent on its way up. A session a finished run closed (asleep,
// paused) is not starting: past a short wait the box says it is ready, since the next message wakes it.

import { describe, expect, it } from "vitest";

import { readinessOf, STARTING_MAX_MS } from "@/components/ask-first/use-agent-readiness";
import type { AgentSelection } from "@/lib/ask-first-model";

const selected = { harness: "claude-code" } as unknown as AgentSelection;
const member = (status: string) => ({ status }) as Parameters<typeof readinessOf>[0];

describe("readinessOf", () => {
  it("is ready for a standby, working or waiting agent, and needs a reconnect when it failed", () => {
    for (const s of ["idle", "working", "waiting"]) expect(readinessOf(member(s), null, selected)).toBe("ready");
    expect(readinessOf(member("failed"), null, selected)).toBe("reconnect");
    expect(readinessOf(undefined, null, selected)).toBe("none");
  });

  it("says Starting while a closed session is new, and ready once it has stayed closed past the wait", () => {
    for (const s of ["asleep", "paused"]) {
      expect(readinessOf(member(s), null, selected, 0)).toBe("starting");
      expect(readinessOf(member(s), null, selected, STARTING_MAX_MS - 1)).toBe("starting");
      expect(readinessOf(member(s), null, selected, STARTING_MAX_MS)).toBe("ready");
    }
  });
});

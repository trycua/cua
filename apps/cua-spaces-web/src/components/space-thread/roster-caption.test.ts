// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { agentCaption } from "./roster";

describe("an agent's chip", () => {
  it("says Working while a turn is under way, never Starting or Installing", () => {
    expect(agentCaption("working", "Starting")).toBe("Working");
    expect(agentCaption("working", "Installing")).toBe("Working");
    expect(agentCaption("working", "Using Terminal")).toBe("Using Terminal");
    expect(agentCaption("working", null)).toBe("Working");
  });

  it("keeps what an idle or waiting agent says", () => {
    expect(agentCaption("waiting", "2 open tasks")).toBe("2 open tasks");
    expect(agentCaption("asleep", null)).toBeNull();
  });
});

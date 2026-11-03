// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Whether the keychain prompt is on screen (macOS; the SwiftUI app's
// SecurityAgentWindow): read only while the launch waits for it.
import { describe, expect, it } from "vitest";
import { SECURITY_AGENT_SCRIPT, SecurityAgentWatcher } from "../src/model/security-agent";

describe("the keychain prompt", () => {
  it("reads the window list while asked to, and forgets it after", async () => {
    let reads = 0;
    const answers = [true, false];
    const w = new SecurityAgentWatcher(async () => answers[reads++] ?? false, 5);
    expect(w.current).toBeNull();
    w.follow(true);
    await new Promise((r) => setTimeout(r, 30));
    expect(reads).toBeGreaterThanOrEqual(2);
    expect(w.current).toBe(false);
    w.follow(false);
    const after = reads;
    await new Promise((r) => setTimeout(r, 20));
    expect(reads).toBe(after);
    expect(w.current).toBeNull();
  });

  it("looks for a SecurityAgent window wider than 100 pt", () => {
    expect(SECURITY_AGENT_SCRIPT).toContain("'SecurityAgent'");
    expect(SECURITY_AGENT_SCRIPT).toContain("> 100");
  });
});

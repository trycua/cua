// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The update channel the app records for its daemon and this machine's host
// (cua_mesh::policy reads `<cua home>/app-update-channel`).
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { CHANNEL_FILE, recordUpdateChannel } from "../src/mesh-channel";

const scratch = mkdtempSync(path.join(tmpdir(), "cua-mesh-channel-test-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

describe("recordUpdateChannel", () => {
  it("writes the channel into the cua home, and follows a switch", () => {
    const home = path.join(scratch, "home", ".cua");
    recordUpdateChannel("beta", home);
    expect(readFileSync(path.join(home, CHANNEL_FILE), "utf8")).toBe("beta\n");
    recordUpdateChannel("stable", home);
    expect(readFileSync(path.join(home, CHANNEL_FILE), "utf8")).toBe("stable\n");
  });

  it("never throws when the home cannot be written", () => {
    expect(() => recordUpdateChannel("beta", "/dev/null/not-a-dir")).not.toThrow();
  });
});

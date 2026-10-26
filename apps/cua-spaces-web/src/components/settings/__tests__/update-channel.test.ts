// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { UpdateState } from "@/bridge";
import { channelStatus } from "../update-channel";

const state = (patch: Partial<UpdateState>): UpdateState => ({ status: "idle", currentVersion: "0.16.0", version: null, percent: null, notesUrl: null, message: null, ...patch });

describe("channelStatus", () => {
  it("says nothing while idle, and that it checks right after a switch", () => {
    expect(channelStatus(state({}), false)).toBeNull();
    expect(channelStatus(state({}), true)).toEqual({ text: "Checking for updates…", ready: false });
  });

  it("follows the download, then offers the restart", () => {
    const version = "0.17.0-nightly.20261025+d45964f";
    expect(channelStatus(state({ status: "downloading", version, percent: 42.7 }), false)?.text).toBe(`Downloading Cua Spaces ${version} (42%)…`);
    expect(channelStatus(state({ status: "downloaded", version }), false)).toEqual({ text: `Cua Spaces ${version} is ready. Restart to start using it.`, ready: true });
  });

  it("says why an update failed", () => {
    expect(channelStatus(state({ status: "error", message: "The feed could not be read" }), false)?.text).toBe("Couldn't update: The feed could not be read");
  });
});

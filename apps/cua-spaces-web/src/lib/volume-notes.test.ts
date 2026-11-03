// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { spaceVolumeErrors } from "@/bridge/volume";

import { volumeUnavailableNote } from "./volume-notes";

describe("why a Space has no Cua Volume", () => {
  it("says the image predates it when its cua-spacesd does", () => {
    expect(volumeUnavailableNote("no Cua Volume in this Space: its cua-spacesd predates the volume")).toBe(
      "Cua Volume isn't available in this Space (its image predates it). Files saved in its volume folder stay in this Space and don't sync.",
    );
  });

  it("names the missing FUSE on runc", () => {
    const note = volumeUnavailableNote("no Cua Volume in this Space: no /dev/fuse (a container on runc)");
    expect(note).toContain("Cua Volume isn't available in this Space: its container can't mount it (no FUSE on runc).");
    expect(note).toContain("don't sync");
  });

  it("passes any other reason through without the daemon's prefix", () => {
    expect(volumeUnavailableNote("Cua Volume not mounted: no answer within 30 s")).toBe(
      "Cua Volume isn't available in this Space: no answer within 30 s. Files saved in its volume folder stay in this Space and don't sync.",
    );
  });

  it("reads volume_errors from sync and mount status, one row per Space", () => {
    const errors = spaceVolumeErrors(
      { device_id: "d", device_name: "Mac", feed: "live", volume_errors: [{ space: "e2e-1005", error: "no /dev/fuse" }] },
      { state: "off", method: "none", volume_errors: [{ space: "e2e-1005", error: "dup" }, { space: "other", error: "x predates y" }] },
    );
    expect(errors.map((e) => e.space)).toEqual(["e2e-1005", "other"]);
    expect(volumeUnavailableNote(errors[0]!.error)).toContain("no FUSE on runc");
    expect(volumeUnavailableNote(errors[1]!.error)).toContain("predates");
    // An older daemon without volume_errors: nothing to say.
    expect(spaceVolumeErrors({ device_id: "d", device_name: "Mac", feed: "live" }, null)).toEqual([]);
  });
});

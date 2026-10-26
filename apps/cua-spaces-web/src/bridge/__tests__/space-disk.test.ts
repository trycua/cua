// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `spaceDiskBytes`: what a Space takes on its computer's disk, for the
// Spaces page's sizes and a bulk delete's "Frees …" (CUA-2204).

import { describe, expect, it } from "vitest";
import { spaceDiskBytes, type ResourcesSnapshot } from "@/bridge";
import { demoResources } from "@/bridge/adapters/demo/resources";
import { demoSpaceRows } from "@/bridge/adapters/demo-data";

const GB = 1024 ** 3;

describe("spaceDiskBytes", () => {
  const snap = demoResources(demoSpaceRows(0), 0);

  it("answers this Mac's Spaces and yours on other computers by this app's id, stopped ones too", () => {
    expect(spaceDiskBytes(snap, "local:design-review")).toBe(19 * GB);
    expect(spaceDiskBytes(snap, "local:agent-sandbox")).toBe(6 * GB);
    expect(spaceDiskBytes(snap, "relay:mac-mini/release-checks")).toBe(21 * GB);
    expect(spaceDiskBytes(snap, "relay:linux-box/windows-legacy")).toBe(31 * GB);
  });

  it("is null when the size isn't known", () => {
    // No numbers yet, or a Space with no entry (shared with you, or its
    // computer is offline or hasn't answered).
    expect(spaceDiskBytes(null, "local:design-review")).toBeNull();
    expect(spaceDiskBytes(snap, "relay:studio-pc/render")).toBeNull();
    // An entry without a disk (a container), or with only an upper bound
    // (its blocks are shared with other copies): not what deleting frees.
    const odd: ResourcesSnapshot = {
      ...snap,
      spaces: [
        { ...snap.spaces[0]!, id: "local:container", diskOnHost: null },
        { ...snap.spaces[0]!, id: "local:clone", diskOnHost: { bytes: null, allocated: 30 * GB, sharesSpace: true } },
        { ...snap.spaces[0]!, id: "local:empty", diskOnHost: { bytes: 0, allocated: 0, sharesSpace: false } },
      ],
    };
    expect(spaceDiskBytes(odd, "local:container")).toBeNull();
    expect(spaceDiskBytes(odd, "local:clone")).toBeNull();
    // Zero is a size.
    expect(spaceDiskBytes(odd, "local:empty")).toBe(0);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { HostError, type DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { THUMBNAIL_REFRESH_MS, ThumbnailStore } from "../thumbnails";

/** A host that answers `spaces.thumbnail` with `answer` and counts the asks. */
function host(answer: (spaceId: string) => unknown) {
  const asked: string[] = [];
  const adapter = {
    mode: "webkit",
    subscribe: () => () => {},
    call: async (op: string, args: { spaceId: string }) => {
      if (op !== "spaces.thumbnail") throw new HostError("no", "unsupported");
      asked.push(args.spaceId);
      return answer(args.spaceId);
    },
  } as unknown as DataAdapter;
  return { adapter, asked };
}

const shot = (id: string, at = 1) => ({ url: `data:image/jpeg;base64,${id}`, capturedAtMs: at });

describe("Space thumbnails", () => {
  it("keeps at most its cap, least recently used first out", () => {
    const t = new ThumbnailStore(createDemoAdapter({ latencyMs: 0 }), Date.now, 3);
    for (const id of ["a", "b", "c"]) t.set(id, shot(id));
    t.touch("a");
    t.set("d", shot("d"));
    expect(["a", "b", "c", "d"].map((id) => t.get(id) !== null)).toEqual([true, false, true, true]);
    expect(t.size).toBe(3);
  });

  it("keeps the newer image, and drops the Spaces that left the list", () => {
    const t = new ThumbnailStore(createDemoAdapter({ latencyMs: 0 }));
    t.set("a", shot("new", 10));
    t.set("a", shot("old", 5));
    expect(t.get("a")).toContain("new");
    t.set("b", shot("b"));
    t.retain(new Set(["b"]));
    expect(t.get("a")).toBeNull();
    expect(t.size).toBe(1);
  });

  it("asks a Space again only after the refresh interval, and never a pending row", async () => {
    let now = 0;
    const { adapter, asked } = host((id) => shot(id));
    const t = new ThumbnailStore(adapter, () => now);
    await t.request("a");
    await t.request("a");
    await t.request("pending:x");
    expect(asked).toEqual(["a"]);
    expect(t.get("a")).toContain("a");
    now = THUMBNAIL_REFRESH_MS;
    await t.request("a");
    expect(asked).toEqual(["a", "a"]);
  });

  it("stops asking a host with no thumbnails", async () => {
    const { adapter, asked } = host(() => {
      throw new HostError("no thumbnails here", "unsupported");
    });
    const t = new ThumbnailStore(adapter);
    await t.request("a");
    await t.request("b");
    expect(asked).toEqual(["a"]);
    expect(t.get("a")).toBeNull();
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A message typed for a home that is still being made shows in its thread at once and leaves when really posted.

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { dropQueue, enqueue, resetQueued, resolveQueue, useQueued } from "./queued";

describe("pending messages of a Space being made", () => {
  beforeEach(() => resetQueued());

  it("shows the message at once, keeps it when the create finishes, and drops it once posted", () => {
    enqueue("pending-1", "@claude hi");
    const early = renderHook(() => useQueued("pending-1"));
    expect(early.result.current.map((q) => q.text)).toEqual(["@claude hi"]);

    resolveQueue("pending-1", "local:brave-otter");
    const created = renderHook(() => useQueued("local:brave-otter"));
    expect(created.result.current.map((q) => q.text)).toEqual(["@claude hi"]);

    dropQueue("pending-1");
    const after = renderHook(() => useQueued("local:brave-otter"));
    expect(after.result.current).toEqual([]);
  });
});

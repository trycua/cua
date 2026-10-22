// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { BridgeContext, type BridgeContextValue } from "../BridgeProvider";
import { useFeatureRecorded } from "../hooks";

function wrapper(track: (batch: unknown[]) => void) {
  const store = { telemetry: { track } };
  const value = { mode: "electron", core: {}, adapter: null, store, ready: Promise.resolve(store) } as unknown as BridgeContextValue;
  return ({ children }: { children?: ReactNode }) => <BridgeContext.Provider value={value}>{children}</BridgeContext.Provider>;
}

describe("useFeatureRecorded", () => {
  it("records the feature once when it opens, not while `when` is false", async () => {
    const track = vi.fn();
    const { rerender } = renderHook(({ when }) => useFeatureRecorded("agents_page_open", when), {
      wrapper: wrapper(track),
      initialProps: { when: false },
    });
    await Promise.resolve();
    expect(track).not.toHaveBeenCalled();
    rerender({ when: true });
    await waitFor(() => expect(track).toHaveBeenCalledTimes(1));
    expect(track).toHaveBeenCalledWith([{ type: "feature", feature: "agents_page_open" }]);
    rerender({ when: true });
    expect(track).toHaveBeenCalledTimes(1);
  });
});

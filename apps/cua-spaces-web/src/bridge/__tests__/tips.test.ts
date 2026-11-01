// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A tip's work done elsewhere (a share, a power change) never gets in that
// work's way: a core without tips, or one that throws, changes nothing.

import { describe, expect, it, vi } from "vitest";
import { sendTipsEvent } from "../tips";
import type { BridgeStore } from "../store";

const store = (tryCall: (m: string) => unknown) => {
  const updateSetting = vi.fn(async () => {});
  const s = {
    core: { status: "ready", tryCall },
    get: () => ({ data: { values: { tips: { off: false, done: [] } } } }),
    updateSetting,
  } as unknown as BridgeStore;
  return { s, updateSetting };
};

describe("sendTipsEvent", () => {
  it("saves what the core makes of the event", async () => {
    const { s, updateSetting } = store(() => ({ off: false, done: ["share"] }));
    await sendTipsEvent(s, { type: "shared" });
    expect(updateSetting).toHaveBeenCalledWith("tips", { off: false, done: ["share"] });
  });

  it("changes nothing on a core without tips or one that throws", async () => {
    for (const tryCall of [() => undefined, () => { throw new Error("no tips.reduce"); }]) {
      const { s, updateSetting } = store(tryCall);
      await expect(sendTipsEvent(s, { type: "shared" })).resolves.toBeUndefined();
      expect(updateSetting).not.toHaveBeenCalled();
    }
  });
});

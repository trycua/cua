// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { FIXTURE_NOW, FIXTURE_SPACES } from "../model/fixtures";
import type { Space } from "../model/types";
import { initialState, reduce } from "./portal";

const cloudSpace = (id: string, lastUsedAt: number): Space => ({
  id,
  name: id,
  os: "linux",
  status: "running",
  detail: "team-a",
  lastUsedAt,
  scene: "linux-terminal",
});

describe("notify", () => {
  it("sets a transient notice with a bumped sequence id, then clears by id", () => {
    const state = initialState();
    const notified = reduce(state, { type: "notify", text: "Creating a Space…" });
    expect(notified.notice).toMatchObject({ kind: "create", text: "Creating a Space…" });
    expect(notified.notice?.id).toBe(state.noticeSeq + 1);
    expect(notified.noticeSeq).toBe(state.noticeSeq + 1);

    // clear-notice only clears the matching id.
    const cleared = reduce(notified, { type: "clear-notice", id: notified.notice!.id });
    expect(cleared.notice).toBeNull();
    const stale = reduce(notified, { type: "clear-notice", id: notified.notice!.id - 1 });
    expect(stale.notice).not.toBeNull();
  });
});

describe("sync-spaces", () => {
  it("replaces the list, keeps the newer local MRU touch, and re-sorts", () => {
    let state = initialState();
    // The user recently switched to Windows QA.
    state = reduce(state, { type: "select", id: "windows-qa", now: FIXTURE_NOW + 60_000 });

    const replacement = [
      { ...FIXTURE_SPACES[0]!, lastUsedAt: FIXTURE_NOW },
      { ...FIXTURE_SPACES[1]!, lastUsedAt: FIXTURE_NOW - 1 }, // stale server timestamp
      cloudSpace("cloud:team-a-x", FIXTURE_NOW + 5_000),
    ];
    const synced = reduce(state, { type: "sync-spaces", spaces: replacement });

    expect(synced.spaces.map((s) => s.id)).toEqual([
      "windows-qa", // local touch (NOW+60s) beats the stale sync timestamp
      "cloud:team-a-x",
      "this-mac",
    ]);
    expect(synced.selectedId).toBe("windows-qa");
  });

  it("falls back to the first Space when the selection disappears", () => {
    const state = initialState();
    const synced = reduce(state, {
      type: "sync-spaces",
      spaces: [cloudSpace("cloud:team-a-y", FIXTURE_NOW)],
    });
    expect(synced.selectedId).toBe("cloud:team-a-y");
    expect(synced.focusIndex).toBe(0);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { FixtureUpdater } from "../src/model/updates";
import { initialUpdateState, reduceUpdate, releaseNotesUrl, UPDATE_POLL_MS, UPDATE_STARTUP_DELAY_MS, type UpdateEvent } from "../src/update-state";

const run = (events: UpdateEvent[], from = initialUpdateState("0.9.2")) => events.reduce(reduceUpdate, from);

describe("the update state machine", () => {
  it("goes idle, checking, available, downloading, downloaded", () => {
    let s = initialUpdateState("0.9.2");
    expect(s).toEqual({ status: "idle", currentVersion: "0.9.2", version: null, percent: null, notesUrl: null, message: null });
    s = reduceUpdate(s, { type: "checking" });
    expect(s.status).toBe("checking");
    s = reduceUpdate(s, { type: "available", version: "0.9.3" });
    expect(s).toMatchObject({ status: "available", version: "0.9.3", percent: null, notesUrl: "https://github.com/trycua/cua/releases/tag/cua-spaces-v0.9.3" });
    s = reduceUpdate(s, { type: "progress", percent: 41.5 });
    expect(s).toMatchObject({ status: "downloading", percent: 41.5 });
    s = reduceUpdate(s, { type: "downloaded", version: "0.9.3" });
    expect(s).toMatchObject({ status: "downloaded", version: "0.9.3", percent: 100, message: null });
  });

  it("keeps a downloaded update through later checks, the same update found again, and failures", () => {
    const ready = run([{ type: "available", version: "0.9.3" }, { type: "downloaded", version: "0.9.3" }]);
    expect(run([{ type: "checking" }], ready)).toBe(ready);
    expect(run([{ type: "none" }], ready)).toBe(ready);
    expect(run([{ type: "available", version: "0.9.3" }], ready)).toBe(ready);
    expect(run([{ type: "progress", percent: 10 }], ready)).toBe(ready);
    expect(run([{ type: "error", message: "offline" }], ready)).toBe(ready);
  });

  it("downloads a newer update over the one waiting", () => {
    const ready = run([{ type: "available", version: "0.9.3" }, { type: "downloaded", version: "0.9.3" }]);
    expect(run([{ type: "available", version: "0.9.4" }], ready)).toMatchObject({ status: "available", version: "0.9.4", percent: null });
  });

  it("a failure keeps the reason, and the next check starts clean", () => {
    const failed = run([{ type: "checking" }, { type: "error", message: "offline" }]);
    expect(failed).toMatchObject({ status: "error", message: "offline" });
    expect(reduceUpdate(failed, { type: "checking" })).toMatchObject({ status: "checking", message: null });
    expect(reduceUpdate(failed, { type: "none" })).toEqual(initialUpdateState("0.9.2"));
  });

  it("does not hide a found or running update behind the next check", () => {
    const found = run([{ type: "available", version: "0.9.3" }]);
    expect(reduceUpdate(found, { type: "checking" })).toBe(found);
  });

  it("keeps the percent in 0 to 100", () => {
    expect(run([{ type: "progress", percent: 140 }]).percent).toBe(100);
    expect(run([{ type: "progress", percent: -3 }]).percent).toBe(0);
  });

  it("points at the repository's release tag", () => {
    expect(releaseNotesUrl(" 0.9.3-rc.4 ")).toBe("https://github.com/trycua/cua/releases/tag/cua-spaces-v0.9.3-rc.4");
  });

  it("checks shortly after launch, and every few hours", () => {
    expect(UPDATE_STARTUP_DELAY_MS).toBe(15_000);
    expect(UPDATE_POLL_MS).toBe(4 * 60 * 60 * 1000);
  });
});

describe("the fixture updater (what the bridge drives)", () => {
  it("restarts only when asked, and only with a download", () => {
    const u = new FixtureUpdater();
    const words: string[] = [];
    u.onUpdateEvent = (w) => words.push(w);
    u.report({ type: "available", version: "0.9.3" }, "found");
    u.installNow();
    expect(u.installs).toBe(0);
    u.report({ type: "progress", percent: 50 });
    u.report({ type: "downloaded", version: "0.9.3" }, "downloaded");
    expect(u.installs).toBe(0);
    u.installNow();
    expect(u.installs).toBe(1);
    expect(words).toEqual(["found", "downloaded", "installed"]);
  });
});

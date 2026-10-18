// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { UpdateState } from "@/bridge";
import { restartWarning, runsInProgress, updateView } from "./update-copy";

const base: UpdateState = { status: "idle", currentVersion: "0.9.2", version: null, percent: null, notesUrl: null, message: null };

describe("updateView", () => {
  it("is quiet while idle, checking or failed", () => {
    expect(updateView(undefined)).toBeNull();
    for (const status of ["idle", "checking", "error"] as const) expect(updateView({ ...base, status, message: "offline" })).toBeNull();
  });

  it("says it is downloading, with the percent when the host has one", () => {
    expect(updateView({ ...base, status: "available", version: "0.9.3" })).toEqual({ kind: "downloading", label: "Downloading update 0.9.3…", percent: null });
    expect(updateView({ ...base, status: "downloading", version: "0.9.3", percent: 42.9 })).toEqual({ kind: "downloading", label: "Downloading update 0.9.3 (42%)", percent: 42 });
    expect(updateView({ ...base, status: "downloading", version: "0.9.3", percent: 140 })?.kind).toBe("downloading");
  });

  it("says it is ready once downloaded", () => {
    expect(updateView({ ...base, status: "downloaded", version: "0.9.3", percent: 100 })).toEqual({ kind: "ready", label: "Update ready", detail: "Cua Spaces 0.9.3" });
    expect(updateView({ ...base, status: "downloaded" })).toEqual({ kind: "ready", label: "Update ready", detail: "A new version of Cua Spaces" });
  });
});

describe("restart guard", () => {
  it("counts only runs that are working", () => {
    expect(runsInProgress(undefined)).toBe(0);
    expect(runsInProgress([{ status: "idle" }, { status: "running" }, { status: "failed" }, { status: "running" }])).toBe(2);
  });

  it("names what a restart would stop", () => {
    expect(restartWarning(1).description).toContain("An agent is still working");
    expect(restartWarning(3).description).toContain("3 agents are still working");
    expect(restartWarning(1)).toMatchObject({ confirm: "Restart anyway", cancel: "Not now" });
  });
});

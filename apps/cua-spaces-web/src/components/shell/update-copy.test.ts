// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { UpdateState } from "@/bridge";
import { restartWarning, runsInProgress, updateTooltip, updateView } from "./update-copy";

const base: UpdateState = { status: "idle", currentVersion: "0.9.2", version: null, percent: null, notesUrl: null, message: null };

describe("updateView", () => {
  it("is quiet while idle or checking", () => {
    expect(updateView(undefined)).toBeNull();
    for (const status of ["idle", "checking"] as const) expect(updateView({ ...base, status })).toBeNull();
  });

  it("says a failed step will be retried", () => {
    expect(updateView({ ...base, status: "error", message: "The network is offline." })).toEqual({ kind: "error", label: "Couldn't update: The network is offline. Cua Spaces tries again later." });
    expect(updateView({ ...base, status: "error" })).toEqual({ kind: "error", label: "Couldn't update. Cua Spaces tries again later." });
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

  it("says it is installing while Restart to update runs", () => {
    expect(updateView({ ...base, status: "downloaded", version: "0.9.3" }, true)).toEqual({ kind: "installing", label: "Installing update 0.9.3…" });
  });

  it("tooltips name the next step when ready", () => {
    expect(updateTooltip({ kind: "ready", label: "Update ready", detail: "Cua Spaces 0.9.3" })).toBe("Update ready: restart to install");
    expect(updateTooltip({ kind: "downloading", label: "Downloading update 0.9.3 (42%)", percent: 42 })).toBe("Downloading update 0.9.3 (42%)");
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

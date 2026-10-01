// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { FIXTURE_NOW, FIXTURE_SPACES } from "./fixtures";
import { clampCount, createFleetLocally, estimateHourly, hourlyRate, resolveMembers } from "./fleet";
import type { FleetDraft } from "./types";

const qa: FleetDraft = {
  templateId: "qa-matrix",
  region: "us-central",
  size: "standard",
  autoSuspend: 15,
  count: 3,
  customOs: "linux",
};

describe("resolveMembers", () => {
  it("returns fixed members for non-adjustable templates regardless of count", () => {
    expect(resolveMembers({ ...qa, count: 8 }).map((m) => m.name)).toEqual(["Windows QA", "Linux QA", "macOS Verify"]);
  });

  it("extends adjustable templates deterministically", () => {
    const members = resolveMembers({ ...qa, templateId: "parallel-build", count: 5 });
    expect(members.map((m) => m.name)).toEqual(["Builder 1", "Builder 2", "Builder 3", "Builder 4", "Builder 5"]);
    expect(members.every((m) => m.os === "linux")).toBe(true);
  });

  it("uses the custom OS for the custom template", () => {
    const members = resolveMembers({ ...qa, templateId: "custom", count: 2, customOs: "windows" });
    expect(members).toEqual([
      { name: "Computer 1", os: "windows", scene: "windows-desktop" },
      { name: "Computer 2", os: "windows", scene: "windows-desktop" },
    ]);
  });

  it("clamps counts into range", () => {
    expect(clampCount(0)).toBe(1);
    expect(clampCount(99)).toBe(8);
    expect(clampCount(Number.NaN)).toBe(1);
  });
});

describe("estimateHourly", () => {
  it("prices by OS multiplier and sums honestly", () => {
    expect(hourlyRate("linux", "standard")).toBe(0.34);
    expect(hourlyRate("windows", "standard")).toBe(0.46);
    expect(hourlyRate("macos", "standard")).toBe(0.71);
    const est = estimateHourly(qa);
    expect(est.computers).toBe(3);
    expect(est.maxHourlyUsd).toBe(1.51);
    expect(est.breakdown).toEqual([
      { os: "linux", count: 1, hourlyUsd: 0.34 },
      { os: "macos", count: 1, hourlyUsd: 0.71 },
      { os: "windows", count: 1, hourlyUsd: 0.46 },
    ]);
  });
});

describe("createFleetLocally", () => {
  it("is deterministic and provisions every member", () => {
    const a = createFleetLocally(qa, FIXTURE_SPACES, FIXTURE_NOW);
    const b = createFleetLocally(qa, FIXTURE_SPACES, FIXTURE_NOW);
    expect(a).toEqual(b);
    expect(a.fleet.id).toBe("fleet-1-qa-matrix");
    expect(a.spaces.map((s) => s.id)).toEqual(["fleet-1-qa-matrix-1", "fleet-1-qa-matrix-2", "fleet-1-qa-matrix-3"]);
    expect(a.spaces.every((s) => s.status === "provisioning" && s.fleetId === a.fleet.id)).toBe(true);
    expect(a.fleet.spaceIds).toEqual(a.spaces.map((s) => s.id));
  });

  it("de-duplicates names that already exist", () => {
    const { spaces } = createFleetLocally(qa, FIXTURE_SPACES, FIXTURE_NOW);
    expect(spaces.map((s) => s.name)).toEqual(["Windows QA 2", "Linux QA", "macOS Verify"]);
  });

  it("increments the fleet ordinal based on existing fleets", () => {
    const first = createFleetLocally(qa, FIXTURE_SPACES, FIXTURE_NOW);
    const second = createFleetLocally(
      { ...qa, templateId: "clean-browsers" },
      [...FIXTURE_SPACES, ...first.spaces],
      FIXTURE_NOW + 1,
    );
    expect(second.fleet.id).toBe("fleet-2-clean-browsers");
  });

  it("stamps new Spaces so they lead the MRU order", () => {
    const { spaces } = createFleetLocally(qa, FIXTURE_SPACES, FIXTURE_NOW + 5000);
    const newest = Math.max(...FIXTURE_SPACES.map((s) => s.lastUsedAt));
    expect(spaces.every((s) => s.lastUsedAt > newest)).toBe(true);
  });
});

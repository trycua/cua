// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// An agent home's grants reach the daemon through a file in the cua home (CUA-2630).
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { cuaHomeOf, homeGrantsOf, writeHomeGrants } from "../src/model/home-grants";

const dir = mkdtempSync(path.join(tmpdir(), "cua-home-grants-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));
const read = () => JSON.parse(readFileSync(path.join(dir, "agent-home", "grants.json"), "utf8"));

describe("a home's grants file", () => {
  it("reads the request's grants and ignores the wrong types", () => {
    expect(homeGrantsOf({ listAll: true, machines: ["b", "a", "a", 3, ""] })).toEqual({ listAll: true, machines: ["a", "b"] });
    expect(homeGrantsOf({ listAll: "yes" })).toBeNull();
    expect(homeGrantsOf(null)).toBeNull();
    expect(homeGrantsOf([])).toBeNull();
  });

  it("is written per Space, owner only, and keeps what the other homes hold", async () => {
    await writeHomeGrants(dir, "local:home-a", { listAll: true });
    await writeHomeGrants(dir, "local:home-b", { machines: ["m-air"] });
    await writeHomeGrants(dir, "local:home-a", { machines: ["m-mini"] });
    expect(read()).toEqual({
      "local:home-a": { list_all: true, machines: ["m-mini"] },
      "local:home-b": { list_all: false, machines: ["m-air"] },
    });
    if (process.platform !== "win32") expect(statSync(path.join(dir, "agent-home", "grants.json")).mode & 0o077).toBe(0);
  });

  it("keeps what later requests @mention, adding to it and never forgetting it", async () => {
    expect(homeGrantsOf({ mentions: ["ci", "local:ci", "ci", 4] })).toEqual({ mentions: ["ci", "local:ci"] });
    await writeHomeGrants(dir, "local:home-m", { mentions: ["ci", "local:ci"] });
    await writeHomeGrants(dir, "local:home-m", { mentions: ["studio"] });
    await writeHomeGrants(dir, "local:home-m", { listAll: true });
    expect(read()["local:home-m"]).toEqual({ list_all: true, machines: [], mentions: ["ci", "local:ci", "studio"] });
  });

  it("uses CUA_HOME, else ~/.cua", () => {
    expect(cuaHomeOf({ CUA_HOME: "/x/y" })).toBe("/x/y");
    expect(cuaHomeOf({})).toMatch(/\.cua$/);
  });
});

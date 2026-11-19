// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// State files that are truncated or garbage must not crash the app or lose
// what they held: the app starts from the defaults and keeps the file as
// `<name>.damaged` (the core does the same for settings.json).
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { forgetKept, keepDamaged } from "../src/damaged";
import { ChecklistModel } from "../src/model/checklist";
import { OnboardingStore } from "../src/model/onboarding";
import type { Native } from "../src/native/load";

const dirs: string[] = [];
const dir = () => {
  const d = mkdtempSync(path.join(tmpdir(), "cua-damaged-"));
  dirs.push(d);
  return d;
};
beforeEach(forgetKept);
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

/** The steps the checklist's words map to (the real ones come from the native core). */
const native = { AppChecklistStep: { AddMachine: "AddMachine", CreateSpace: "CreateSpace", Teleport: "Teleport", Share: "Share", RunAgent: "RunAgent" } } as unknown as Native;

const GARBAGE = ["{", '{"completed": tru', "", "null", "[]", "\u0000\u0001garbage", '"a string"'];

describe("keepDamaged", () => {
  it("copies the file beside itself, once per content, and says so", () => {
    const d = dir();
    const file = path.join(d, "x.json");
    writeFileSync(file, "{ truncated");
    const said: string[] = [];
    expect(keepDamaged(file, "is damaged", (m) => said.push(m))).toBe(`${file}.damaged`);
    expect(readFileSync(`${file}.damaged`, "utf8")).toBe("{ truncated");
    expect(said).toHaveLength(1);
    expect(said[0]).toContain(`${file}.damaged`);
    // Read again (the app reads state files often): not copied or said again.
    expect(keepDamaged(file, "is damaged", (m) => said.push(m))).toBe(`${file}.damaged`);
    expect(said).toHaveLength(1);
  });

  it("keeps the latest damaged file when it changes", () => {
    const d = dir();
    const file = path.join(d, "x.json");
    writeFileSync(file, "first");
    keepDamaged(file, "is damaged", () => {});
    writeFileSync(file, "second, longer");
    keepDamaged(file, "is damaged", () => {});
    expect(readFileSync(`${file}.damaged`, "utf8")).toBe("second, longer");
  });

  it("keeps a private file private, and never throws for a missing one", () => {
    if (process.platform === "win32") return;
    const d = dir();
    const file = path.join(d, "secret.json");
    writeFileSync(file, "{", { mode: 0o600 });
    keepDamaged(file, "is damaged", () => {});
    expect(statSync(`${file}.damaged`).mode & 0o777).toBe(0o600);
    expect(keepDamaged(path.join(d, "missing.json"))).toBeNull();
  });
});

describe("the first run's state", () => {
  it.each(GARBAGE)("starts the first run again for %j, and keeps the file", (text) => {
    const d = dir();
    const s = new OnboardingStore(path.join(d, "onboarding.json"));
    writeFileSync(s.file, text);
    expect(s.read()).toEqual({ completed: false, mode: "client" });
    expect(readFileSync(`${s.file}.damaged`, "utf8")).toBe(text);
    // Finishing writes a good file over it; the damaged copy stays.
    s.finish("host");
    expect(s.read()).toEqual({ completed: true, mode: "host" });
    expect(existsSync(`${s.file}.damaged`)).toBe(true);
  });

  it("does not keep anything for a missing or a good file", () => {
    const d = dir();
    const s = new OnboardingStore(path.join(d, "onboarding.json"));
    expect(s.read().completed).toBe(false);
    s.finish("client");
    expect(s.read().completed).toBe(true);
    expect(existsSync(`${s.file}.damaged`)).toBe(false);
  });
});

describe("the checklist", () => {
  it.each(GARBAGE)("starts empty for %j, and keeps the file", (text) => {
    const d = dir();
    const file = path.join(d, "checklist.json");
    writeFileSync(file, text);
    const m = new ChecklistModel(native, file);
    expect(m.state).toEqual({ done: [], dismissed: false, shown: false, skipped: [] });
    expect(readFileSync(`${file}.damaged`, "utf8")).toBe(text);
  });

  it("reads a good file and keeps nothing", () => {
    const d = dir();
    const file = path.join(d, "checklist.json");
    writeFileSync(file, JSON.stringify({ done: ["create_space"], dismissed: true, shown: true, skipped: [] }));
    const m = new ChecklistModel(native, file);
    expect(m.state.done).toEqual(["CreateSpace"]);
    expect(m.state.dismissed).toBe(true);
    expect(existsSync(`${file}.damaged`)).toBe(false);
  });

  it("does not copy the Swift app's damaged seed file (it is not ours)", () => {
    const d = dir();
    const seed = path.join(d, "seed.json");
    writeFileSync(seed, "{");
    const m = new ChecklistModel(native, path.join(d, "checklist.json"), seed);
    expect(m.state.done).toEqual([]);
    expect(existsSync(`${seed}.damaged`)).toBe(false);
  });
});

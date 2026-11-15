// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The first run's saved state (the SwiftUI app's OnboardingModel completed,
// finish and restart): the same onboarding.json, and "Show again".
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { OnboardingStore } from "../src/model/onboarding";

describe("the first run's state", () => {
  const dirs: string[] = [];
  const store = () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cua-onboarding-"));
    dirs.push(dir);
    return new OnboardingStore(path.join(dir, "state", "onboarding.json"));
  };
  afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

  it("is due until it finishes, then saves the Swift app's file", () => {
    const s = store();
    expect(s.read()).toEqual({ completed: false, mode: "client" });
    let told = 0;
    s.subscribe(() => told++);
    s.finish("host");
    expect(JSON.parse(readFileSync(s.file, "utf8"))).toEqual({ completed: true, mode: "host" });
    expect(s.read()).toEqual({ completed: true, mode: "host" });
    expect(told).toBe(1);
  });

  it("reads a damaged file as due", () => {
    const s = store();
    s.finish("client");
    writeFileSync(s.file, "{");
    expect(s.read().completed).toBe(false);
  });

  it("Show again makes it due until it finishes again, and leaves the file", () => {
    const s = store();
    s.finish("client");
    s.restart();
    expect(s.read().completed).toBe(false);
    expect(JSON.parse(readFileSync(s.file, "utf8")).completed).toBe(true);
    s.finish("client");
    expect(s.read().completed).toBe(true);
  });

  it("Reset & Show forgets the saved first run, and only that file, as the core's plan lists", () => {
    const s = store();
    s.finish("host");
    const beside = path.join(path.dirname(s.file), "checklist.json");
    writeFileSync(beside, "{}");
    let told = 0;
    s.subscribe(() => told++);
    s.restart();
    s.reset();
    expect(existsSync(s.file)).toBe(false);
    expect(existsSync(beside)).toBe(true);
    expect(s.read()).toEqual({ completed: false, mode: "client" });
    expect(told).toBe(2);
    // Twice is the same as once, and a finish after it is saved as usual.
    s.reset();
    expect(s.read()).toEqual({ completed: false, mode: "client" });
    s.finish("client");
    expect(s.read()).toEqual({ completed: true, mode: "client" });
    // The file it removes is the one the app core's reset plan names.
    const plan = JSON.parse(readFileSync(path.join(__dirname, "../../../libs/cua/crates/cua-spaces-app-core/contracts/onboarding-reset.json"), "utf8")) as {
      hostFiles: string[];
      firstStep: string;
    };
    expect(plan.hostFiles).toEqual([path.basename(s.file)]);
    expect(plan.firstStep).toBe("welcome");
  });
});

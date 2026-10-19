// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { answerRequest, pendingRequests } from "../src/bridge/nudges";

const dirs: string[] = [];
const fresh = () => {
  const d = mkdtempSync(path.join(tmpdir(), "nudges-"));
  dirs.push(d);
  return d;
};
const request = (dir: string, id: string, expiresMs: number) =>
  writeFileSync(path.join(dir, `${id}.json`), JSON.stringify({ id, agent: "Codex", action: "uninstall an app", atMs: 1, expiresMs }));

afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

describe("the risky-action question's files", () => {
  it("lists waiting requests and writes the answer next to one", () => {
    const dir = fresh();
    request(dir, "risky-1-100", 5_000);
    request(dir, "risky-2-200", 500);
    expect(pendingRequests(dir, 1_000).map((r) => r.id)).toEqual(["risky-1-100"]);
    answerRequest(dir, "risky-1-100", "separate");
    expect(readFileSync(path.join(dir, "risky-1-100.answer"), "utf8")).toBe("separate");
    expect(pendingRequests(dir, 1_000)).toEqual([]);
  });

  it("leaves anything else alone", () => {
    const dir = fresh();
    answerRequest(dir, "risky-3-300", "here");
    expect(existsSync(path.join(dir, "risky-3-300.answer"))).toBe(false);
    request(dir, "risky-4-400", 5_000);
    answerRequest(dir, "../risky-4-400", "here");
    answerRequest(dir, "risky-4-400", "delete");
    expect(existsSync(path.join(dir, "risky-4-400.answer"))).toBe(false);
    expect(pendingRequests(path.join(dir, "missing"))).toEqual([]);
  });
});

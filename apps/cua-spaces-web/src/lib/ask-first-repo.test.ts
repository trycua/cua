// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { chooseRepo, clearRepo, parseRepo, resetRepos } from "./ask-first-repo";

const repo = (url: string, label: string) => ({ url, label });

describe("what a typed repo means", () => {
  it("reads owner/name, links to the repo or to one of its pages, and ssh links", () => {
    const want = repo("https://github.com/trycua/demo-failing-tests", "trycua/demo-failing-tests");
    expect(parseRepo("trycua/demo-failing-tests")).toEqual(want);
    expect(parseRepo("  https://github.com/trycua/demo-failing-tests  ")).toEqual(want);
    expect(parseRepo("https://github.com/trycua/demo-failing-tests.git")).toEqual(want);
    expect(parseRepo("github.com/trycua/demo-failing-tests")).toEqual(want);
    expect(parseRepo("https://github.com/trycua/demo-failing-tests/pull/12")).toEqual(want);
    expect(parseRepo("https://github.com/trycua/demo-failing-tests/tree/main/tests?x=1")).toEqual(want);
    expect(parseRepo("git@github.com:trycua/demo-failing-tests.git")).toEqual(want);
    expect(parseRepo("https://gitlab.com/group/project")).toEqual(repo("https://gitlab.com/group/project", "group/project"));
  });

  it("refuses what is not a repo", () => {
    for (const bad of ["", "   ", "trycua", "a b/c", "https://github.com/trycua", "trycua/..", "../etc/passwd", "ftp://x", "just some words about a repo"]) {
      expect(parseRepo(bad), bad).toBeNull();
    }
  });
});

describe("the repo of the next task", () => {
  beforeEach(() => {
    localStorage.clear();
    resetRepos();
  });

  it("is kept, with the ones used lately newest first and each once, until it is removed", () => {
    chooseRepo(repo("https://github.com/a/one", "a/one"));
    chooseRepo(repo("https://github.com/b/two", "b/two"));
    chooseRepo(repo("https://github.com/a/one", "a/one"));
    const saved = JSON.parse(localStorage.getItem("cua.askFirst.repos")!);
    expect(saved.current.label).toBe("a/one");
    expect(saved.recent.map((r: { label: string }) => r.label)).toEqual(["a/one", "b/two"]);
    clearRepo();
    const after = JSON.parse(localStorage.getItem("cua.askFirst.repos")!);
    expect(after.current).toBeNull();
    expect(after.recent).toHaveLength(2);
  });

  it("keeps five", () => {
    for (let i = 0; i < 8; i++) chooseRepo(repo(`https://github.com/o/r${i}`, `o/r${i}`));
    expect(JSON.parse(localStorage.getItem("cua.askFirst.repos")!).recent).toHaveLength(5);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Agents tab's Not started section: each run has a Remove, the section a Clear all, and a run whose Space is gone
// leaves by itself. Only the run's record on this device is removed. And a poll that finds nothing new redraws nothing.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { WakeNote } from "@/components/space-thread/summon";
import { CREATE_ROW_GRACE_MS, ORPHAN_MAX_MS, orphanedRuns } from "@/hooks/use-agent-attempts";
import { addAskTask, askTasks, resetAskTasks, type AskTask } from "@/lib/ask-first";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
});
afterEach(cleanup);

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const found = (s: string, timeout = 5000) =>
  waitFor(
    () => {
      const el = q(s);
      expect(el).not.toBeNull();
      return el!;
    },
    { timeout },
  );
const click = async (el: HTMLElement) => {
  await act(async () => {
    fireEvent.click(el);
    await new Promise((r) => setTimeout(r, 20));
  });
};
const open = () => mountApp("/agents", { failsCreates: false, listsGhost: false });
const failed = (prompt: string, over: Partial<AskTask> = {}) =>
  addAskTask({ prompt, agent: "claude-code", os: "linux", inSpace: false, error: "boom", ...over });

describe("Not started", () => {
  it.skipIf(!wasmBuilt)("each run has a Remove that drops only its record; Clear all drops the rest", async () => {
    const a = failed("fix the flaky checkout tests");
    failed("bump the retry timeout");
    await open();
    await waitFor(() => expect(qa("[data-chat-run]")).toHaveLength(2), { timeout: 5000 });
    expect(q("[data-agent-section='Not started']")?.textContent).toContain("Clear all");
    const first = qa("[data-chat-run]").find((r) => r.textContent?.includes("fix the flaky"))!;
    const x = first.querySelector<HTMLElement>("[data-chat-remove]")!;
    expect(x.getAttribute("aria-label")).toBe("Remove fix the flaky checkout tests from the list");
    // Reachable by keyboard: a real button, not hover only.
    expect(x.tagName).toBe("BUTTON");
    await click(x);
    await waitFor(() => expect(qa("[data-chat-run]")).toHaveLength(1));
    expect(askTasks().some((t) => t.id === a.id)).toBe(false);
    expect(askTasks()).toHaveLength(1);
    // Agents in Spaces are not touched.
    expect(qa("[data-chat]:not([data-chat-run])").length).toBeGreaterThan(0);
    await click(await found("[data-runs-clear]"));
    await waitFor(() => expect(q("[data-agent-section='Not started']")).toBeNull());
    expect(askTasks()).toHaveLength(0);
  });

  it.skipIf(!wasmBuilt)("a run whose Space is gone leaves the list by itself", async () => {
    failed("run in a deleted computer", { spaceId: "relay:gone/vm", createdMs: Date.now() - 10 * 60_000 });
    failed("a fresh failure", { createdMs: Date.now() });
    await open();
    await waitFor(() => expect(askTasks().map((t) => t.prompt)).toEqual(["a fresh failure"]), { timeout: 5000 });
    await waitFor(() => expect(qa("[data-chat-run]")).toHaveLength(1));
  });
});

describe("what counts as orphaned", () => {
  const now = 10 * ORPHAN_MAX_MS;
  const task = (over: Partial<AskTask>): AskTask =>
    ({
      id: "t",
      prompt: "x",
      agent: "claude-code",
      os: "linux",
      createdMs: now - 1,
      pendingId: null,
      spaceId: null,
      runId: null,
      error: "boom",
      ending: null,
      inSpace: false,
      ...over,
    }) as AskTask;
  const spaces = [{ id: "local:a" }];

  it("a failed run whose real Space is gone, once past the grace; never a pending one, never while the list is empty", () => {
    const old = now - CREATE_ROW_GRACE_MS - 1;
    expect(orphanedRuns([task({ spaceId: "local:zzz", createdMs: old })], {}, spaces, now).taskIds).toEqual(["t"]);
    expect(orphanedRuns([task({ spaceId: "local:a", createdMs: old })], {}, spaces, now).taskIds).toEqual([]);
    expect(orphanedRuns([task({ spaceId: "pending:q", createdMs: old })], {}, spaces, now).taskIds).toEqual([]);
    expect(orphanedRuns([task({ spaceId: "local:zzz", createdMs: now - 1000 })], {}, spaces, now).taskIds).toEqual([]);
    expect(orphanedRuns([task({ spaceId: "local:zzz", createdMs: old })], {}, [], now).taskIds).toEqual([]);
    expect(orphanedRuns([task({ spaceId: "local:zzz", createdMs: old })], {}, undefined, now).taskIds).toEqual([]);
  });

  it("a run that never started in a week; a started run never", () => {
    expect(orphanedRuns([task({ createdMs: now - ORPHAN_MAX_MS - 1, spaceId: "local:a" })], {}, spaces, now).taskIds).toEqual(["t"]);
    expect(orphanedRuns([task({ createdMs: now - ORPHAN_MAX_MS - 1, runId: "r1" })], {}, spaces, now).taskIds).toEqual([]);
    expect(orphanedRuns([task({ createdMs: now - ORPHAN_MAX_MS - 1, ending: "kept" })], {}, spaces, now).taskIds).toEqual([]);
  });

  it("notes: Space gone or a week old; one being connected is left", () => {
    const note = (id: string, over: Partial<WakeNote>): WakeNote => ({
      id,
      seq: null,
      text: "@x",
      cant: {} as never,
      state: "blocked",
      atMs: now - ORPHAN_MAX_MS / 2,
      ...over,
    });
    const o = orphanedRuns(
      [],
      {
        "local:zzz": [note("gone", {}), note("busy", { state: "connecting" })],
        "local:a": [note("ok", {}), note("ancient", { atMs: now - ORPHAN_MAX_MS - 1 })],
      },
      spaces,
      now,
    );
    expect(o.notes).toEqual([
      { spaceId: "local:zzz", id: "gone" },
      { spaceId: "local:a", id: "ancient" },
    ]);
  });
});

describe("the list stays still", () => {
  it.skipIf(!wasmBuilt)("a poll that finds nothing new redraws nothing and never says it is checking", async () => {
    await open();
    await found("[data-chat]:not([data-chat-run])");
    await waitFor(() => expect(q("[data-agents-loading]")).toBeNull());
    // Opening the first thread marks it read: let that answer come in (one pass) before watching the next.
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
      await new Promise((r) => setTimeout(r, 1500));
    });
    const list = q("[data-agent-list]")!;
    const rows = qa("[data-chat]");
    let changes = 0;
    const seenLoading: boolean[] = [];
    const mo = new MutationObserver((m) => {
      changes += m.length;
      seenLoading.push(Boolean(q("[data-agents-loading]")));
    });
    mo.observe(list, { subtree: true, childList: true, attributes: true, characterData: true });
    // The page coming back to the front reads every Space again at once, as each poll does.
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
      await new Promise((r) => setTimeout(r, 1500));
    });
    mo.disconnect();
    expect(changes).toBe(0);
    expect(seenLoading).not.toContain(true);
    expect(qa("[data-chat]")).toEqual(rows);
  });
});

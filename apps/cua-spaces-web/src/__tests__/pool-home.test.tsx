// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Home, option A ("Machine grid"): your computers as one supercomputer. The total at the top, one card per
// computer with its free room and its jobs, a big Add a computer tile, and Run something, which places the job on
// the computer with the most free room (the app core's `pool.place`) and moves it when that computer can't be reached.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetWakeNotes } from "@/components/space-thread/summon";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { askTasks, resetAskTasks, setThisMacConfirmed } from "@/lib/ask-first";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetWakeNotes();
  resetConnectState();
  resetConnectedAppsState();
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const q = <T extends HTMLElement = HTMLElement>(s: string, root: ParentNode = document) => root.querySelector<T>(s);
const qa = (s: string, root: ParentNode = document) => [...root.querySelectorAll<HTMLElement>(s)];
const found = <T extends HTMLElement = HTMLElement>(s: string, timeout = 8000) =>
  waitFor(
    () => {
      const el = q<T>(s);
      expect(el).toBeTruthy();
      return el!;
    },
    { timeout },
  );
/** What it reads, with the no-break spaces around Home's dots as plain ones. */
const words = (el: Element | null | undefined) => (el?.textContent ?? "").replace(/\u00a0/g, " ");
const card = (id: string) => found(`[data-pool-computer='${id}']`);

/** The demo's computers, keeping only `ids` (the demo has This Mac, the Mac mini, the Linux box and the offline Studio PC). */
const onlyMachines =
  (ids: string[]): NonNullable<TestHost["answer"]> =>
  (op, out) => {
    if (op === "machines.list" && Array.isArray(out)) return out.filter((m: { id: string }) => ids.includes(m.id));
    if (op === "resources.get" && out && typeof out === "object") {
      const snap = out as { computers: { id: string }[] };
      return { ...snap, computers: snap.computers.filter((c) => ids.includes(c.id)) };
    }
    return out;
  };

const HOST: TestHost = { failsCreates: false, listsGhost: false, demo: { desktops: true } };

describe.skipIf(!wasmBuilt)("Home: your computers as one supercomputer", () => {
  it("with only This Mac: one card, the total, and Add a computer saying why you'd add one", async () => {
    await mountApp("/home", { ...HOST, demo: { desktops: false }, answer: onlyMachines(["this-mac"]) });
    const head = await found("[data-pool-headline]");
    await waitFor(() => expect(words(head)).toBe("1 computer · 12 cores · 64 GB"));
    expect(qa("[data-pool-computer]")).toHaveLength(1);
    const here = await card("this-mac");
    expect(here.dataset.current).toBe("true");
    expect(q("[data-pool-presence]", here)?.textContent).toBe("Online");
    await waitFor(() => expect(q("[data-pool-free]", here)?.dataset.poolFree).not.toBe("none"));
    const tile = await found("[data-pool-add-tile]");
    expect(tile.textContent).toContain("Add a computer");
    expect(tile.textContent).toContain("agents run there while you keep working on this one");
    // The tile and the header button both open Add a computer.
    fireEvent.click(tile);
    await waitFor(() => expect(document.querySelector("[role='dialog']")).not.toBeNull());
  });

  it("three computers with one offline: the total counts all three, the offline one has no free room and says why", async () => {
    await mountApp("/home", { ...HOST, answer: onlyMachines(["this-mac", "mac-mini", "studio-pc"]) });
    const head = await found("[data-pool-headline]");
    // This Mac (12 cores, 64 GB) and the Mac mini (8, 16 GB) sent numbers; the Studio PC sent none.
    await waitFor(() => expect(words(head)).toBe("3 computers · 20 cores · 80 GB"));
    expect(q("[data-pool-line]")?.textContent).toMatch(/^2 online now, with \d+ cores and \d+ GB free for new jobs\.$/);
    const pc = await card("studio-pc");
    expect(pc.dataset.online).toBe("false");
    expect(q("[data-pool-presence]", pc)?.textContent).toMatch(/^Offline/);
    expect(q("[data-pool-free]", pc)?.dataset.poolFree).toBe("none");
    expect(q("[data-pool-empty]", pc)?.textContent).toBe("Can’t reach it. New jobs go to your other computers until it’s back.");
    const mini = await card("mac-mini");
    expect(mini.dataset.online).toBe("true");
    expect(words(q("[data-pool-free]", mini))).toMatch(/Free now\d of 8 cores · 9 GB/);
    // A computer's name opens its own page.
    expect(q<HTMLAnchorElement>("[data-pool-computer-link]", mini)?.getAttribute("href")).toBe("/spaces/computer/mac-mini");
  });

  it("jobs running: each agent shows on the card of the computer it works on, and opens its Space's thread", async () => {
    const { router } = await mountApp("/home", HOST);
    const mini = await card("mac-mini");
    // The demo's Hermes works in release-checks, a Space on the Mac mini.
    const job = await waitFor(() => {
      const el = q("[data-pool-job]", mini);
      expect(el).toBeTruthy();
      return el!;
    });
    expect(job.textContent).toContain("release-checks");
    expect(q("[data-pool-job]", await card("studio-pc"))).toBeNull();
    fireEvent.click(job);
    await waitFor(() => expect(decodeURIComponent(router.state.location.pathname)).toBe("/spaces/relay:mac-mini/release-checks"));
  });

  it("Run something: picks the computer with the most room, lets you change it, and starts the job there without leaving Home", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const { router } = await mountApp("/home", { ...HOST, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    fireEvent.click(await found("[data-pool-run]"));
    const sheet = await found("[data-run-sheet]");
    // Claude Code and Codex; the demo is signed in to Codex only, so Codex is picked.
    expect(qa("[data-run-agent]", sheet).map((b) => b.dataset.runAgent)).toEqual(["claude-code", "openai-codex"]);
    await waitFor(() => expect(q("[data-run-agent='openai-codex']", sheet)?.getAttribute("aria-checked")).toBe("true"));
    // Where: the Mac mini has the most free room (this Mac goes last, the Studio PC is offline).
    await waitFor(() => expect(q("[data-run-where][aria-checked='true']", sheet)?.dataset.runWhere).toBe("mac-mini"));
    expect(q("[data-run-where='mac-mini']", sheet)?.textContent).toMatch(/cores free, best fit/);
    expect(q("[data-run-where='this-mac']", sheet)?.dataset.reason).toBe("you-are-here");
    expect(q<HTMLButtonElement>("[data-run-where='studio-pc']", sheet)?.disabled).toBe(true);
    const submit = q<HTMLButtonElement>("[data-run-submit]", sheet)!;
    expect(submit.disabled).toBe(true);
    expect(submit.textContent).toBe("Run on Mac mini");
    // You can change it.
    fireEvent.click(q("[data-run-where='linux-box']", sheet)!);
    await waitFor(() => expect(submit.textContent).toBe("Run on Linux box"));
    fireEvent.click(q("[data-run-where='mac-mini']", sheet)!);
    fireEvent.change(q("[data-run-task]", sheet)!, { target: { value: "Make the search page load faster" } });
    await waitFor(() => expect(submit.disabled).toBe(false));
    await act(async () => {
      fireEvent.click(submit);
    });
    // The job is placed on the Mac mini, in its shared desktop, and Home stays put.
    await waitFor(() =>
      expect(askTasks()[0]).toMatchObject({
        prompt: "Make the search page load faster",
        agent: "openai-codex",
        computer: { id: "mac-mini", name: "Mac mini" },
      }),
    );
    expect(askTasks()[0]!.spaceId).toBe("relay:mac-mini");
    expect(askTasks()[0]!.movedFrom).toBeUndefined();
    await waitFor(() => expect(q("[data-run-sheet]")).toBeNull());
    expect(router.state.location.pathname).toBe("/home");
    await waitFor(() => expect(calls.some((c) => c.args?.spaceId === "relay:mac-mini" && /^thread\.|^agents\.start$/.test(c.op))).toBe(true));
  });

  it("a computer that can't be reached: the job moves to the next best one, and Home says where", async () => {
    const { router } = await mountApp("/home", {
      ...HOST,
      intercept: (op, args) => {
        const spaceId = (args as { spaceId?: string } | undefined)?.spaceId;
        if (spaceId === "relay:mac-mini" && (op === "thread.post" || op === "agents.start" || op === "thread.agentAdd"))
          throw new Error("Mac mini is unreachable: connection refused");
        return undefined;
      },
    });
    fireEvent.click(await found("[data-pool-run]"));
    const sheet = await found("[data-run-sheet]");
    await waitFor(() => expect(q("[data-run-where][aria-checked='true']", sheet)?.dataset.runWhere).toBe("mac-mini"));
    fireEvent.change(q("[data-run-task]", sheet)!, { target: { value: "Fix the flaky checkout tests" } });
    const submit = q<HTMLButtonElement>("[data-run-submit]", sheet)!;
    await waitFor(() => expect(submit.disabled).toBe(false));
    await act(async () => {
      fireEvent.click(submit);
    });
    // The start on the Mac mini fails as unreachable; the pool's next pick, the Linux box, takes it.
    await waitFor(() => expect(askTasks()[0]).toMatchObject({ movedFrom: { id: "mac-mini" }, computer: { id: "linux-box" }, error: null }), {
      timeout: 10_000,
    });
    const moved = await found("[data-pool-moved]");
    expect(moved.textContent).toContain("Mac mini couldn’t be reached, so “Fix the flaky checkout tests” moved to Linux box.");
    fireEvent.click(q("[data-pool-moved-show]", moved)!);
    await waitFor(() => expect(decodeURIComponent(router.state.location.pathname)).toBe("/spaces/relay:linux-box"));
    // Dismissed, it stays away.
    await act(async () => void router.navigate({ to: "/home" }));
    fireEvent.click(q("[data-pool-moved] button[aria-label='Dismiss']") ?? (await found("[data-pool-moved] button[aria-label='Dismiss']")));
    await waitFor(() => expect(q("[data-pool-moved]")).toBeNull());
  });

  it("the computer goes offline between picking and starting: nothing is handed to its agent, the job moves, and Home says where", async () => {
    // Online while the sheet is open and the Mac mini is picked; switched off just before Run is pressed. The relay
    // still lists it online, and posting to its thread still succeeds (the thread lives on the relay): only
    // `spaces.reach`'s health check finds it gone.
    // Two computers: the Mac mini (most room, picked) and This Mac, which takes the job when it moves.
    setThisMacConfirmed(true);
    let miniOff = false;
    const handed: string[] = [];
    await mountApp("/home", {
      ...HOST,
      answer: onlyMachines(["this-mac", "mac-mini"]),
      intercept: (op, args) => {
        const spaceId = (args as { spaceId?: string } | undefined)?.spaceId;
        if (op === "spaces.reach" && spaceId === "relay:mac-mini" && miniOff)
          return { reachable: false, listedOffline: false, reason: "relay:mac-mini did not answer in 4 s" };
        if (spaceId === "relay:mac-mini" && (op.startsWith("thread.") || op === "agents.start")) handed.push(op);
        return undefined;
      },
    });
    fireEvent.click(await found("[data-pool-run]"));
    const sheet = await found("[data-run-sheet]");
    await waitFor(() => expect(q("[data-run-where][aria-checked='true']", sheet)?.dataset.runWhere).toBe("mac-mini"));
    fireEvent.change(q("[data-run-task]", sheet)!, { target: { value: "Bisect the slow build" } });
    const submit = q<HTMLButtonElement>("[data-run-submit]", sheet)!;
    await waitFor(() => expect(submit.disabled).toBe(false));
    miniOff = true;
    await act(async () => {
      fireEvent.click(submit);
    });
    await waitFor(() => expect(askTasks()[0]).toMatchObject({ movedFrom: { id: "mac-mini" }, computer: { id: "this-mac" }, error: null }), {
      timeout: 10_000,
    });
    expect(askTasks()[0]!.onHost).toBe(true);
    // It stays there: This Mac took it.
    await act(async () => void (await new Promise((r) => setTimeout(r, 1_500))));
    expect(askTasks()[0]).toMatchObject({ movedFrom: { id: "mac-mini" }, computer: { id: "this-mac" }, error: null });
    // Nothing went to the Mac mini's thread or its agent: the job did not queue for a computer that is gone.
    expect(handed.filter((op) => op === "thread.post" || op === "thread.agentAdd" || op === "agents.start")).toEqual([]);
    const moved = await found("[data-pool-moved]");
    expect(moved.textContent).toContain("Mac mini couldn’t be reached, so “Bisect the slow build” moved to This Mac.");
  });

  it("a computer that answers takes the job: it is asked first, then the job goes to it", async () => {
    const calls: { op: string; spaceId?: string }[] = [];
    await mountApp("/home", {
      ...HOST,
      intercept: (op, args) => (calls.push({ op, spaceId: (args as { spaceId?: string } | undefined)?.spaceId }), undefined),
    });
    fireEvent.click(await found("[data-pool-run]"));
    const sheet = await found("[data-run-sheet]");
    await waitFor(() => expect(q("[data-run-where][aria-checked='true']", sheet)?.dataset.runWhere).toBe("mac-mini"));
    fireEvent.change(q("[data-run-task]", sheet)!, { target: { value: "Update the changelog" } });
    const submit = q<HTMLButtonElement>("[data-run-submit]", sheet)!;
    await waitFor(() => expect(submit.disabled).toBe(false));
    await act(async () => {
      fireEvent.click(submit);
    });
    // The job goes to it (its thread or a run), and only after it was asked.
    const handedAt = () => calls.findIndex((c) => c.spaceId === "relay:mac-mini" && /^thread\.(post|agentAdd)$|^agents\.start$/.test(c.op));
    await waitFor(() => expect(handedAt()).toBeGreaterThanOrEqual(0), { timeout: 10_000 });
    const handed = handedAt();
    const reach = calls.findIndex((c) => c.op === "spaces.reach" && c.spaceId === "relay:mac-mini");
    expect(reach).toBeGreaterThanOrEqual(0);
    expect(reach).toBeLessThan(handed);
    expect(askTasks()[0]).toMatchObject({ computer: { id: "mac-mini" }, error: null });
    expect(askTasks()[0]!.movedFrom).toBeUndefined();
  });

  it("the task you typed stays through This Mac's confirm and Pick another, and clears once the job starts", async () => {
    setThisMacConfirmed(false);
    await mountApp("/home", HOST);
    fireEvent.click(await found("[data-pool-run]"));
    let sheet = await found("[data-run-sheet]");
    await waitFor(() => expect(q<HTMLButtonElement>("[data-run-where='this-mac']", sheet)?.disabled).toBe(false));
    fireEvent.click(q("[data-run-where='this-mac']", sheet)!);
    fireEvent.change(q("[data-run-task]", sheet)!, { target: { value: "Tidy my Downloads folder" } });
    let submit = q<HTMLButtonElement>("[data-run-submit]", sheet)!;
    await waitFor(() => expect(submit.disabled).toBe(false));
    await act(async () => {
      fireEvent.click(submit);
    });
    // "Work on This Mac?": the task is still there behind it.
    await screen.findByRole("button", { name: "Pick another" });
    expect(q<HTMLTextAreaElement>("[data-run-task]")?.value).toBe("Tidy my Downloads folder");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Pick another" }));
    });
    // Back on the sheet, with the task as it was, and nothing started.
    sheet = await found("[data-run-sheet]");
    await waitFor(() => expect(q<HTMLTextAreaElement>("[data-run-task]", sheet)?.value).toBe("Tidy my Downloads folder"));
    expect(askTasks()).toHaveLength(0);
    // Run again and confirm: the job starts with that task, and the field is empty for the next one.
    fireEvent.click(q("[data-run-where='this-mac']", sheet)!);
    submit = q<HTMLButtonElement>("[data-run-submit]", sheet)!;
    await waitFor(() => expect(submit.disabled).toBe(false));
    await act(async () => {
      fireEvent.click(submit);
    });
    await act(async () => {
      fireEvent.click(await screen.findByRole("button", { name: /^Use / }));
    });
    await waitFor(() => expect(askTasks()[0]).toMatchObject({ prompt: "Tidy my Downloads folder", onHost: true }));
    fireEvent.click(await found("[data-pool-run]"));
    sheet = await found("[data-run-sheet]");
    expect(q<HTMLTextAreaElement>("[data-run-task]", sheet)?.value).toBe("");
  });
});

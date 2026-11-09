// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The host's push card and Cua's own entries in a Space's thread: the card is
// the app core's (`threadCards.exportCard`), drawn with the diff tokens; an
// answered card is one muted line; Cua shows as Cua with one APP tag and the
// app icon, and an agent's approval can never draw as a card.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DEMO_PUSH_CARD, DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { threadEntryFromTool } from "@/bridge/ops/thread";
import { mountApp, stubBrowserApis } from "./app-harness";
import { wasmBuilt } from "@/bridge/__tests__/testCore";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;

describe("a card is only ever the host's", () => {
  const entry = (kind: string) => ({
    seq: 1,
    id: "x",
    ts_ms: 1,
    author: { kind, id: kind === "system" ? "system:host" : "agent:claude", name: "n" },
    kind: "approval",
    text: "ask",
    approval: { action: "export:repo.push", details: "d", state: "open", card: { repo: "acme/app" } },
  });
  it("keeps the card from Cua's host and drops it from an agent", () => {
    expect(threadEntryFromTool(entry("system"))!.approval!.card).toEqual({ repo: "acme/app" });
    expect(threadEntryFromTool(entry("agent"))!.approval!.card).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("the host's push card in the thread", () => {
  const first = () => qa("[data-approval]").find((a) => a.querySelector("[data-export-card]"))!;

  it("is compact: headline, branch into base, one diff line, the account and the buttons; the changes are behind Show changes", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadCards: true } });
    const approval = await waitFor(() => {
      const c = first();
      expect(c).toBeTruthy();
      return c;
    });
    const lines = [...approval.querySelectorAll("[data-card-line]")];
    expect(lines.map((l) => (l as HTMLElement).dataset.cardLine)).toEqual(["headline", "branch", "diff", "account", "expander", "buttons"]);
    expect(lines.length).toBeLessThanOrEqual(6);
    expect(approval.querySelector("[data-card-line=headline]")!.textContent).toBe("Claude Code wants to push a branch and open a pull request on acme/app.");
    expect(approval.querySelector("[data-branch-line]")!.textContent).toBe("fix/retry-backoff into main");
    expect(approval.querySelector("[data-totals]")!.textContent).toContain("5 files");
    expect(approval.querySelector("[data-totals] [data-diff='add']")!.textContent).toBe("+32");
    expect(approval.querySelector("[data-totals] [data-diff='del']")!.textContent).toBe("-19");
    expect(approval.querySelector("[data-diff='add']")!.className).toContain("text-diff-add");
    expect(approval.querySelector("[data-diff='del']")!.className).toContain("text-diff-del");
    expect(approval.querySelector("[data-account]")!.textContent).toBe("Cua pushes it from this computer as ada, signed in with gh.");
    // Collapsed: no files, commits or preview, and the headline is not said twice.
    expect(approval.querySelector("[data-file]")).toBeNull();
    expect(approval.querySelector("[data-commit]")).toBeNull();
    expect(approval.querySelector("[data-pr]")).toBeNull();
    expect(approval.textContent!.split("wants to push").length - 1).toBe(1);
    expect(approval.textContent).not.toMatch(/Will open a pull request|Built in|holds no GitHub access/);
    // The button names what happens.
    const buttons = [...approval.querySelectorAll("[data-card-line=buttons] button")].map((b) => b.textContent);
    expect(buttons).toEqual(["Push and open PR", "Not now"]);
    expect(document.body.textContent).not.toContain("\u00b7");
  });

  it("opens to the first three files with their bars, the commits and a pull request header", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadCards: true } });
    const approval = await waitFor(() => {
      const c = first();
      expect(c).toBeTruthy();
      return c;
    });
    await act(async () => {
      fireEvent.click(approval.querySelector("[data-card-expander]")!);
    });
    expect(approval.querySelector("[data-card-expander]")!.textContent).toBe("Hide changes");
    const files = [...approval.querySelectorAll("[data-file]")] as HTMLElement[];
    expect(files).toHaveLength(3);
    expect(files.map((f) => f.dataset.status)).toEqual(["modified", "added", "deleted"]);
    // 9 added, 3 removed: four green squares and one red.
    expect(files[0]!.querySelector("[data-bar]")!.getAttribute("data-bar")).toBe("add,add,add,add,del");
    expect(files[0]!.querySelectorAll("[data-bar] > span")).toHaveLength(5);
    expect(approval.textContent).toContain("+2 more");
    expect([...approval.querySelectorAll("[data-commit]")].map((c) => (c as HTMLElement).dataset.commit)).toEqual(
      DEMO_PUSH_CARD.commits.map((c) => c.sha.slice(0, 7)),
    );
    const pr = approval.querySelector("[data-pr]")!;
    expect(pr.textContent).toContain("Back off between reconnect retries");
    expect(pr.querySelector("[data-pr-state]")!.textContent).toBe("Open");
    expect(pr.querySelectorAll("p").length).toBeLessThanOrEqual(3);
    expect(pr.textContent).not.toContain("into main");
  });

  it("a push of only a branch says Push and shows no pull request", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadCards: true } });
    const card = await waitFor(() => {
      const c = qa("[data-approval]").find((a) => a.textContent?.includes("acme/docs") && a.querySelector("[data-export-card]"));
      expect(c).toBeTruthy();
      return c!;
    });
    expect([...card.querySelectorAll("[data-card-line=buttons] button")].map((b) => b.textContent)).toEqual(["Push", "Not now"]);
    await act(async () => {
      fireEvent.click(card.querySelector("[data-card-expander]")!);
    });
    expect(card.querySelector("[data-pr]")).toBeNull();
    expect(card.querySelectorAll("[data-file]")).toHaveLength(1);
  });

  it("after a yes the card is one muted line", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadCards: true } });
    const approval = await waitFor(() => {
      const c = first();
      expect(c).toBeTruthy();
      return c;
    });
    const seq = approval.dataset.approval!;
    await act(async () => {
      fireEvent.click([...approval.querySelectorAll("button")].find((b) => b.textContent === "Push and open PR")!);
      await new Promise((r) => setTimeout(r, 20));
    });
    await waitFor(() => expect(q(`[data-approval='${seq}'][data-approval-collapsed]`)).not.toBeNull());
    const done = q(`[data-approval='${seq}']`)!;
    expect(done.textContent).toBe("Approved the push and pull request of fix/retry-backoff to acme/app by Ada");
    expect(done.querySelector("[data-export-card]")).toBeNull();
  });

  it("shows Cua's own entries as Cua, tagged APP, with the app icon; agents and people keep how they show", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadCards: true } });
    await waitFor(() => expect(qa("[data-author-tag]").length).toBeGreaterThan(0));
    const tags = qa("[data-author-tag]");
    expect(tags.every((t) => t.textContent === "APP")).toBe(true);
    // One tag per Cua entry, never beside an agent or a person.
    for (const t of tags) {
      const row = t.closest("[data-entry]")!;
      expect(row.querySelector("img[alt='Cua app']")).not.toBeNull();
      expect(row.textContent).toContain("Cua");
    }
    const agentRows = qa("[data-entry]").filter((r) => r.querySelector("[data-author-tag]") === null);
    expect(agentRows.length).toBeGreaterThan(0);
    expect(q("[data-thread]")!.textContent).not.toMatch(/SYSTEM/);
    expect(document.body.textContent).not.toContain("·");
  });
});

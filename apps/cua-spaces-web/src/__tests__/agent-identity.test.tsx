// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Who an agent in a Space thread is (CUA-2262): its harness's mark, the AI
// tag, its owner's badge, the owner after the tag when two agents share a
// name, and the card its name opens (owner and computer checked by Cua, how
// it joined, the harness, its rights, Remove from Space for who may).

import { existsSync } from "node:fs";
import { join } from "node:path";

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { memberIdentity, type AgentMember, type MemberIdentity, type ThreadEntry } from "@/bridge/ops/thread";
import { APP_MARKS } from "@/components/agents/harness-mark";
import { HARNESSES, harnessInfo } from "@/components/space-thread/harness-marks";
import { joinedWhen, reportedHarness } from "@/components/space-thread/identity";
import { buildRoster } from "@/components/space-thread/roster";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
const app = (demo: Record<string, unknown> = {}) => mountApp(path, { failsCreates: false, listsGhost: false, demo });

const ADA = { id: "acct_ada", name: "Ada" };
const LINUS = { id: "acct_linus", name: "Linus" };
const identity = (over: Partial<MemberIdentity> = {}): MemberIdentity => ({
  origin: "external",
  handle: "claude",
  displayName: "Claude Code",
  collides: false,
  icon: "harness",
  harness: { id: "claude-code", version: "2.4.1", reported: true },
  owner: ADA,
  device: { id: "dev", name: "Ada's MacBook Pro" },
  joined: { method: "prompt", atMs: 0, by: ADA },
  state: "active",
  lastSeenMs: null,
  ...over,
});
const member = (name: string, i: MemberIdentity | null): AgentMember => ({ name, harness: i?.harness.id ?? "claude-code", status: "idle", activity: null, sessions: 1, rights: null, youCan: null, heldBy: null, identity: i });
const roster = (members: AgentMember[], opts: { me?: string; role?: "owner" | "admin" | "member" | "guest" | null; entries?: ThreadEntry[] } = {}) =>
  buildRoster({ members, shares: [], entries: opts.entries ?? [], me: opts.me ?? ADA.id, identity: null, role: opts.role ?? null, harnessName: (h) => h });

/** Opens an agent's card from its name in the thread. */
async function openCard(handle: string): Promise<HTMLElement> {
  const name = await waitFor(() => {
    const el = q(`[data-author-name='${handle}']`);
    expect(el).not.toBeNull();
    return el!;
  });
  await act(async () => {
    fireEvent.click(name);
    await new Promise((r) => setTimeout(r, 20));
  });
  return waitFor(() => {
    const card = q(`[data-provenance='${handle}']`);
    expect(card).not.toBeNull();
    return card!;
  });
}
/** The card's Details, opened: label to value. */
async function details(card: HTMLElement): Promise<Record<string, string>> {
  await act(async () => {
    fireEvent.click(card.querySelector("[data-card-details]")!);
  });
  return Object.fromEntries([...card.querySelectorAll<HTMLElement>("[data-card-details-open] dt")].map((dt) => [dt.textContent!, dt.nextElementSibling!.textContent!]));
}
const line = (card: HTMLElement) => card.querySelector("[data-card-line='who']")?.textContent;

describe("an agent's identity", () => {
  it("draws the harness's mark, the AI tag and the owner's badge in the thread and in In this Space", async () => {
    await app();
    await waitFor(() => expect(q("[data-member='agent:hermes'] [data-owner-badge]")).not.toBeNull());
    // Every agent has its owner's badge, in the list and on its posts.
    for (const [handle, owner] of [
      ["hermes", "acct_ada"],
      ["claude", "acct_grace"],
      ["cursor", "acct_linus"],
    ]) {
      expect(q(`[data-member='agent:${handle}'] [data-owner-badge]`)?.dataset.ownerBadge).toBe(owner);
      expect(q(`[data-member='agent:${handle}'] [data-ai-tag]`)).not.toBeNull();
    }
    expect(q("[data-member='agent:hermes'] [data-harness-mark]")?.dataset.harnessMark).toBe("hermes");
    expect(q("[data-member='agent:cursor'] [data-harness-mark]")?.dataset.harnessMark).toBe("cursor");
    expect(q("[data-member='agent:claude'] [data-harness-mark]")?.dataset.harnessMark).toBe("claude-code");
    // A connected agent that isn't watching the thread says it waits for you; the asleep Hermes is quieter but keeps its badge.
    expect(q("[data-member='agent:cursor']")?.textContent).toContain("Waits for you to message it");
    expect(q("[data-member='agent:hermes']")?.textContent).toContain("Runs in this Space");
    expect(q("[data-member='agent:hermes'] [data-agent-tile]")?.className).toContain("opacity-45");
    expect(q("[data-member='agent:hermes'] [data-owner-badge]")?.closest(".opacity-45")).toBeNull();
    // People get no tag.
    expect(qa("[data-members] [data-member]").filter((m) => !m.dataset.member!.startsWith("agent:")).some((m) => m.querySelector("[data-ai-tag]"))).toBe(false);
    const post = qa("[data-entry]").find((e) => e.querySelector("[data-author-name='claude']"))!;
    expect(post.querySelector("[data-owner-badge]")?.getAttribute("data-owner-badge")).toBe("acct_grace");
    expect(post.querySelector("[data-ai-tag]")?.textContent).toBe("AI");
    // No owner after the tag while the name is used once.
    expect(q("[data-owner-suffix]")).toBeNull();
  });

  it("shows the owner after the tag when two agents use the same name", async () => {
    await app({ threadLookalike: true });
    await waitFor(() => expect(q("[data-member='agent:claude-2']")).not.toBeNull());
    expect(q("[data-member='agent:claude'] [data-owner-suffix]")?.textContent).toBe("(Grace)");
    expect(q("[data-member='agent:claude-2'] [data-owner-suffix]")?.textContent).toBe("(Linus)");
    // It reports no harness: the bot glyph, not Claude Code's mark.
    expect(q("[data-member='agent:claude-2'] [data-harness-mark]")?.dataset.harnessMark).toBe("bot");
    const post = qa("[data-entry]").find((e) => e.querySelector("[data-author-name='claude-2']"))!;
    expect(post.querySelector("[data-owner-suffix]")?.textContent).toBe("(Linus)");
    // The suffix follows the tag.
    const tag = post.querySelector("[data-ai-tag]")!;
    expect(tag.nextElementSibling?.hasAttribute("data-owner-suffix")).toBe(true);
  });

  it("collision rule: same name (any case) or the daemon's flag; one name alone stays plain", () => {
    const r = roster([
      member("claude", identity()),
      member("claude-2", identity({ handle: "claude-2", displayName: "claude code", owner: LINUS })),
      member("codex", identity({ handle: "codex", displayName: "Codex", harness: { id: "codex", version: null, reported: true } })),
      member("hermes", identity({ handle: "hermes", displayName: "Hermes", collides: true })),
    ]);
    expect(r.agents.filter((a) => a.collides).map((a) => a.name)).toEqual(["claude", "claude-2", "hermes"]);
  });

  it("takes the owner from the member's record, never from what the agent's post claims", () => {
    // The post's snapshot says Ada; the record says Linus: the record wins.
    const claim = identity({ handle: "claude-2", owner: ADA });
    const r = roster([member("claude-2", identity({ handle: "claude-2", owner: LINUS }))]);
    expect(r.author({ kind: "agent", id: "agent:claude-2", name: "claude-2", identity: claim }).identity?.owner).toEqual(LINUS);
    // A removed member's posts keep how they posted.
    expect(roster([]).author({ kind: "agent", id: "agent:gone", name: "gone", identity: claim }).identity?.owner).toEqual(ADA);
    // No owner in the daemon's JSON: no identity at all, so no badge is guessed.
    expect(memberIdentity({ handle: "x", display_name: "X" })).toBeNull();
    // An icon is the harness's or an https image, nothing else.
    expect(memberIdentity({ handle: "x", owner: ADA, icon: "javascript:alert(1)" })?.icon).toBeNull();
    expect(memberIdentity({ handle: "x", owner: ADA, icon: "https://example.com/i.png" })?.icon).toBe("https://example.com/i.png");
  });

  it("opens the card from the name: Grace's Claude Code, in one line, with the rest under Details", async () => {
    await app();
    const card = await openCard("claude");
    expect(card.querySelector("[data-card-title]")?.textContent).toBe("Claude Code");
    expect(card.querySelector("[data-card-badge]")?.textContent).toBe("Agent");
    expect(line(card)).toBe("Run by Grace on Grace's MacBook Pro, uses Anthropic");
    // Details is folded until asked.
    expect(card.querySelector("[data-card-details-open]")).toBeNull();
    expect(await details(card)).toMatchObject({
      Handle: "@claude",
      Runs: "Claude Code 2.4.1",
      Joined: expect.stringMatching(/^Connected by Grace with a prompt, \d+ (hour|day)s? ago$/),
    });
    // The owner is a person (their mark and name), never an id.
    expect(card.querySelector("[data-card-owner-open]")?.getAttribute("data-card-owner-open")).toBe("Grace");
    expect(card.textContent).not.toContain("acct_grace");
    // The owner may change who can task, stop and approve it: the selects, under Details.
    expect(card.querySelectorAll("[data-agent-right]")).toHaveLength(3);
    // The old card's facts and fine print are gone.
    expect(card.textContent).not.toContain("checked by Cua");
  });

  it("the card of your own Hermes, started here: run by you in this Space", async () => {
    await app();
    const card = await openCard("hermes");
    expect(line(card)).toBe("Run by you in this Space, uses Anthropic");
    expect(await details(card)).toMatchObject({ Handle: "@hermes", Runs: "Hermes Agent 0.9.2", Joined: expect.stringMatching(/^Started in this Space/) });
  });

  it("a look-alike's card: its own owner, its own handle, Runs not reported", async () => {
    await app({ threadLookalike: true });
    const card = await openCard("claude-2");
    expect(line(card)).toMatch(/^Run by Linus on linus-thinkpad/);
    expect(await details(card)).toMatchObject({ Handle: "@claude-2", Runs: "Not reported" });
  });

  it("Remove from Space: the owner sees it and removing leaves its posts with their mark and owner", async () => {
    await app({ threadLookalike: true });
    const card = await openCard("claude-2");
    const remove = card.querySelector<HTMLElement>("[data-remove-agent='claude-2']")!;
    expect(remove).not.toBeNull();
    await act(async () => {
      fireEvent.click(remove);
      await new Promise((r) => setTimeout(r, 50));
    });
    await waitFor(() => expect(q("[data-member='agent:claude-2']")).toBeNull());
    const note = await waitFor(() => {
      const n = q("[data-member-note='removed'][data-member-handle='claude-2']");
      expect(n).not.toBeNull();
      return n!;
    });
    expect(note.textContent).toMatch(/^Ada removed Claude Code\. Its key no longer works here\. \d\d:\d\d/);
    // Its post stays, with its mark and Linus's badge.
    const post = qa("[data-entry]").find((e) => e.textContent?.includes("deploy key"))!;
    expect(post.querySelector("[data-owner-badge]")?.getAttribute("data-owner-badge")).toBe("acct_linus");
    // Grace's Claude Code is the only one with the name now.
    await waitFor(() => expect(q("[data-member='agent:claude'] [data-owner-suffix]")).toBeNull());
  });

  it("Remove from Space by role: a member sees it only for their own agent", () => {
    const grace = { id: "acct_grace", name: "Grace" };
    const ada = roster([], { me: ADA.id, role: "member" });
    expect(ada.canRemove(identity({ owner: ADA }))).toBe(true);
    expect(ada.canRemove(identity({ owner: grace }))).toBe(false);
    expect(roster([], { me: ADA.id, role: "guest" }).canRemove(identity({ owner: grace }))).toBe(false);
    expect(roster([], { me: ADA.id, role: "admin" }).canRemove(identity({ owner: grace }))).toBe(true);
    expect(roster([], { me: ADA.id, role: "owner" }).canRemove(identity({ owner: grace }))).toBe(true);
  });

  it("a member's card has no Remove for someone else's agent", async () => {
    await app({ threadRole: "member" });
    const card = await openCard("claude");
    expect(card.querySelector("[data-remove-agent]")).toBeNull();
    expect(line(card)).toMatch(/^Run by Grace /);
    // A member reads who may task it, stop it and approve it; only the owner and admins change it.
    const d = await details(card);
    expect(d).toMatchObject({ "Can give it tasks": "Members", "Can stop it": "Admins", "Can approve its steps": "Grace" });
    expect(card.querySelectorAll("[data-agent-right]")).toHaveLength(0);
  });

  it("words the harness and when it joined", () => {
    const now = new Date(2026, 9, 24, 17, 0).getTime();
    expect(joinedWhen(now - 3 * 60_000, now)).toBe("3 minutes ago");
    expect(joinedWhen(new Date(2026, 9, 24, 8, 12).getTime(), now)).toBe("Today 08:12");
    expect(joinedWhen(new Date(2026, 9, 23, 16, 5).getTime(), now)).toBe("Yesterday 16:05");
    expect(reportedHarness(identity({ harness: { id: "codex", version: "0.61.0", reported: true } }))).toBe("Codex CLI 0.61.0");
    expect(reportedHarness(identity({ harness: { id: "other", version: null, reported: false } }))).toBeNull();
    expect(reportedHarness(identity({ origin: "space", harness: { id: "hermes", version: "0.9.2", reported: false } }))).toBe("Hermes Agent 0.9.2");
  });
});

describe("harness marks", () => {
  const pub = join(__dirname, "..", "..", "public");
  it("lists Hermes, Claude Code, Codex, Cursor, OpenClaw, T3 Code in that order", () => {
    expect(HARNESSES.map((h) => h.id)).toEqual(["hermes", "claude-code", "codex", "cursor", "openclaw", "t3code"]);
    expect(harnessInfo("openai-codex")?.id).toBe("codex");
    expect(harnessInfo("something-else")).toBeNull();
  });

  it("every harness uses the coding-app list's official mark (one map, sources in its NOTICE)", () => {
    for (const h of HARNESSES) {
      expect(h.id in APP_MARKS, h.id).toBe(true);
      expect(h.mark?.src, h.id).toBe(APP_MARKS[h.id]!.light);
      expect(existsSync(join(pub, h.mark!.src.slice(1))), h.id).toBe(true);
    }
    // No second folder of marks.
    expect(existsSync(join(pub, "harness-marks"))).toBe(false);
  });
});

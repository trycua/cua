// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Agents tab as a chat (CUA-2266): every agent across Spaces with
// Needs you pinned, one row per agent even in several Spaces, and the DM:
// a view of its Space threads between you and it, where sending is an
// @mention task and approvals are answered in place.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { chatKey, mergeChats } from "@/bridge/agents-dm";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { demoDmEntries } from "@/bridge/adapters/demo/thread";
import type { AgentMember, MemberIdentity } from "@/bridge/ops/thread";
import { needText } from "@/components/agents/agent-chat";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const UBUNTU = "relay:linux-box/ubuntu-build";
const RELEASE = "relay:mac-mini/release-checks";
const DESIGN = "local:design-review";
// Each agent in each Space is its own entry, keyed on its Space and handle there.
const HERMES = `${UBUNTU}/hermes`;
const HERMES_RELEASE = `${RELEASE}/hermes`;
const CODEX = `${DESIGN}/codex`;
const CLAUDE = `${UBUNTU}/claude`;
const CURSOR = `${UBUNTU}/cursor`;
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
const open = (search = "", demo: Record<string, unknown> = {}) => mountApp(`/agents${search}`, { failsCreates: false, listsGhost: false, demo });

describe("the Agents tab", () => {
  it.skipIf(!wasmBuilt)("lists every agent once, Needs you pinned, running first, with owners and unread", async () => {
    await open();
    await found(`[data-chat='${CODEX}']`);
    // Needs you: Hermes's tag, Codex's question, Claude Code's push.
    expect(
      qa("[data-needs-you]")
        .map((n) => n.dataset.needsYou)
        .sort(),
    ).toEqual(["approval", "approval", "question"]);
    expect(qa("[data-needs-you]").map((n) => n.textContent)).toContainEqual(expect.stringContaining("Which branch should the snapshots go to"));
    // Hermes is in release-checks and ubuntu-build: one row for each.
    expect(qa(`[data-chat='${HERMES}']`)).toHaveLength(1);
    expect(qa(`[data-chat='${HERMES_RELEASE}']`)).toHaveLength(1);
    expect(qa("[data-agent-section='Running'] [data-chat]").map((r) => r.dataset.chat)).toEqual(expect.arrayContaining([HERMES_RELEASE, CODEX, CLAUDE]));
    expect(qa("[data-agent-section='Others'] [data-chat]").map((r) => r.dataset.chat)).toEqual(expect.arrayContaining([HERMES, CURSOR]));
    expect(q("[data-agent-section='Others'] [data-chat-count], [data-agent-section='Others'] h2")?.textContent).toContain("2");
    // A teammate's agent says whose it is; yours don't.
    expect(q(`[data-chat='${CLAUDE}']`)?.textContent).toContain("Grace's");
    expect(q(`[data-chat='${CODEX}']`)?.textContent).not.toContain("'s");
    expect(q(`[data-chat='${CODEX}'] [data-chat-line]`)?.textContent).toBe("Waiting for you in design-review");
    expect(Number(q(`[data-chat='${CODEX}'] [data-chat-unread]`)?.dataset.chatUnread)).toBeGreaterThan(0);
    expect(q(`[data-chat='${CODEX}'] [data-owner-badge]`)?.dataset.ownerBadge).toBe("acct_ada");
  });

  it.skipIf(!wasmBuilt)("one agent in two Spaces is two entries, each opening its own Space's thread", async () => {
    await open();
    await found(`[data-chat='${HERMES}']`);
    expect(qa(`[data-chat='${HERMES}']`)).toHaveLength(1);
    expect(qa(`[data-chat='${HERMES_RELEASE}']`)).toHaveLength(1);
    expect(q(`[data-chat='${HERMES}'] [data-chat-line]`)?.textContent).toContain("ubuntu-build");
    expect(q(`[data-chat='${HERMES_RELEASE}'] [data-chat-line]`)?.textContent).toContain("release-checks");
    // The Space tabs are gone: each thread is one Space's.
    await click(q(`[data-chat='${HERMES}']`)!);
    const dm = await found(`[data-dm='${HERMES}'][data-dm-space='${UBUNTU}']`);
    expect(dm.querySelector("[aria-label='Space']")).toBeNull();
    await waitFor(() => expect(q("[data-dm-log]")?.textContent).toContain("Two bullet points"));
    expect(q("[data-dm-space-label]")).toBeNull();
    expect(q("[data-dm-target]")?.dataset.dmTarget).toBe(UBUNTU);
    await click(q(`[data-chat='${HERMES_RELEASE}']`)!);
    await found(`[data-dm='${HERMES_RELEASE}'][data-dm-space='${RELEASE}']`);
    // Its approval, answered right here.
    const card = await found("[data-approval-state='open']");
    expect(card.textContent).toContain("Tag v0.9.4 and publish a release");
    await click([...card.querySelectorAll("button")].find((b) => b.textContent === "Approve")!);
    await found("[data-approval-state='approved']");
  });

  it.skipIf(!wasmBuilt)("sending is an @mention task in the Space the composer names; members see it", async () => {
    await open(`?agent=${encodeURIComponent(CODEX)}`);
    await found(`[data-dm='${CODEX}']`);
    expect(q("[data-dm-members-see]")?.textContent).toBe("Members see it");
    expect(q("[data-dm-target]")?.textContent).toBe("design-review");
    const box = (await found("[data-composer]")) as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: "main, please", selectionStart: 12 } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 30));
    });
    const task = await waitFor(() => {
      const t = qa("[data-dm-log] [data-task]").find((x) => x.textContent?.includes("main, please"));
      expect(t).toBeDefined();
      return t!;
    });
    expect(task.textContent).toContain("@codex main, please");
    // Codex picks it up in design-review and answers under it.
    await waitFor(() => expect(task.dataset.taskState).toBe("done"), { timeout: 5000 });
  });

  it.skipIf(!wasmBuilt)("a teammate's agent at work: the working line with Watch, its push waiting for approval", async () => {
    await open(`?agent=${encodeURIComponent(CLAUDE)}`);
    const line = await found("[data-dm-working]");
    expect(line.textContent).toContain("Bumping the retry timeout in ubuntu-build");
    expect(line.querySelector("[data-dm-watch]")).not.toBeNull();
    expect(q("[data-dm-open-space]")?.textContent).toContain("ubuntu-build");
  });

  it.skipIf(!wasmBuilt)("when only its owner may task it, the composer says so and offers the Space", async () => {
    await open(`?agent=${encodeURIComponent("acct_grace/hermes/hermes")}`, { threadRole: "member" });
    const locked = await found("[data-dm-locked]");
    expect(locked.textContent).toContain("can give Hermes tasks.");
    expect(locked.querySelector("[data-dm-open-space]")?.textContent).toBe("Open ubuntu-build");
    expect(q("[data-composer]")).toBeNull();
  });

  it("no agents: the box to start one, not an empty state with buttons", async () => {
    await open("", { noThread: true });
    await found("[data-agents-unset] [data-ask-box]");
    expect(q("[data-agents-empty]")).toBeNull();
    expect(document.body.textContent).not.toContain("Who should work for you?");
    expect(document.body.textContent).not.toContain("Start one in a Space");
    expect(q("[data-agents-show-all]")).not.toBeNull();
  });

  it.skipIf(!wasmBuilt)("New agent clears the selection: no popup, the unset view with the box", async () => {
    const { router } = await open(`?agent=${encodeURIComponent(CODEX)}`);
    await found(`[data-dm='${CODEX}']`);
    await click(await found("[data-new-agent]"));
    await found("[data-agents-unset] [data-ask-box]");
    expect(q("[data-dm]")).toBeNull();
    expect(q("[data-add-agent]")).toBeNull();
    expect(q("[role='dialog']")).toBeNull();
    expect((router.state.location.search as { new?: boolean }).new).toBe(true);
    expect(q(`[data-chat='${CODEX}']`)?.getAttribute("aria-current")).toBeNull();
    // Picking an entry opens it again.
    await click(q(`[data-chat='${CODEX}']`)!);
    await found(`[data-dm='${CODEX}']`);
    expect(q("[data-agents-unset]")).toBeNull();
  });

  it.skipIf(!wasmBuilt)("ⓘ shows who the agent is in its Space", async () => {
    await open(`?agent=${encodeURIComponent(HERMES)}`);
    await click(await found("[data-dm-info]"));
    const page = await found(`[data-agent-page='${HERMES}']`);
    expect(page.querySelectorAll("[data-agent-page-space]")).toHaveLength(1);
    expect(page.querySelector("[data-provenance='hermes']")).not.toBeNull();
    expect(page.querySelector("[data-agent-page-all]")).not.toBeNull();
  });

  it.skipIf(!wasmBuilt)("the thread's card links to the agent's DM (Activity, under Details)", async () => {
    await mountApp(`/spaces/${encodeURIComponent("relay:linux-box/ubuntu-build")}`, { failsCreates: false, listsGhost: false });
    const name = await found("[data-author-name='claude']");
    await click(name);
    await click(await found("[data-profile-card='agent'] [data-card-details]"));
    await click(await found("[data-agent-activity='claude']"));
    await found(`[data-dm='${CLAUDE}']`);
  });
});

describe("the DM and the list, as data", () => {
  const ident = (owner: string, harness: string, name: string): MemberIdentity => ({
    origin: "space",
    handle: name.toLowerCase(),
    displayName: name,
    collides: false,
    icon: "harness",
    harness: { id: harness, version: null, reported: false },
    owner: { id: owner, name: owner },
    device: null,
    joined: { method: "started", atMs: 0, by: null },
    state: "active",
    lastSeenMs: null,
  });
  const member = (name: string, i: MemberIdentity | null, over: Partial<AgentMember> = {}): AgentMember => ({
    name,
    harness: i?.harness.id ?? "",
    status: "idle",
    activity: null,
    sessions: 1,
    rights: null,
    youCan: null,
    heldBy: null,
    identity: i,
    ...over,
  });
  const space = (id: string) => ({ id, name: id.split(":").pop()!, os: "linux" as const }) as never;

  it.skipIf(!wasmBuilt)("keys one agent across Spaces on owner, harness and name; without an identity, on its Space", async () => {
    const core = await testCore();
    expect(chatKey("a", member("hermes-2", ident("ada", "hermes", "Hermes")))).toBe("ada/hermes/hermes");
    expect(chatKey("local:x", member("bot", null))).toBe("local:x/bot");
    const chats = mergeChats(core, [
      { space: space("local:a"), role: "owner", members: [member("hermes", ident("ada", "hermes", "Hermes"), { status: "asleep" })] },
      {
        space: space("local:b"),
        role: "owner",
        members: [
          member("hermes-2", ident("ada", "hermes", "Hermes"), { status: "working", activity: "drafting notes", dm: { unread: 2, last: null }, needsYou: [] }),
        ],
      },
      { space: space("local:b"), role: "owner", members: [member("hermes-3", ident("grace", "hermes", "Hermes"))] },
    ]);
    expect(chats.map((c) => [c.key, c.spaces.length, c.tone, c.line, c.unread])).toEqual([
      ["ada/hermes/hermes", 2, "working", "Drafting notes in b", 2],
      ["grace/hermes/hermes", 1, "idle", "In b", 0],
    ]);
    // The Agents tab lists each agent in each Space on its own.
    const per = mergeChats(
      core,
      [
        { space: space("local:a"), role: "owner", members: [member("hermes", ident("ada", "hermes", "Hermes"), { status: "asleep" })] },
        {
          space: space("local:b"),
          role: "owner",
          members: [member("hermes-2", ident("ada", "hermes", "Hermes"), { status: "working", activity: "drafting notes" })],
        },
      ],
      [],
      true,
    );
    expect(per.map((c) => [c.key, c.spaces.length, c.tone, c.line]).sort()).toEqual([
      ["local:a/hermes", 1, "asleep", "Asleep in a"],
      ["local:b/hermes-2", 1, "working", "Drafting notes in b"],
    ]);
  });

  it("the DM rule (as the daemon's): the agent's entries, mine to it, and what hangs off them", () => {
    const e = (seq: number, author: string, extra: Record<string, unknown> = {}) => ({
      seq,
      kind: "message",
      text: "",
      mentions: [],
      reply_to: null,
      task: null,
      update: null,
      answer: null,
      author: { id: author },
      ...extra,
    });
    const entries = [
      e(1, "acct_ada", { mentions: ["codex"], task: { to: "codex" } }),
      e(2, "acct_ada", { mentions: ["hermes"], task: { to: "hermes" } }),
      e(3, "system:dispatcher", { kind: "task_update", update: { task: 1 } }),
      e(4, "agent:codex", { reply_to: 1 }),
      e(5, "agent:hermes", { reply_to: 2 }),
      e(6, "acct_grace", { reply_to: 4 }),
      e(7, "system:members", { kind: "system", event: { member: { handle: "codex" } } }),
    ];
    expect(demoDmEntries(entries, "acct_ada", "codex").map((x) => x.seq)).toEqual([1, 3, 4, 6, 7]);
    expect(needText("@ada which branch, `main`?")).toBe("Which branch, main?");
  });
});

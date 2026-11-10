// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { beforeEach, describe, expect, it } from "vitest";

import type { ThreadEntry } from "@/bridge/ops/thread";

import { ABILITY_IDS, NO_GRANTS, NO_MENTIONS } from "./abilities";
import { AGENT_HOME_IMAGE, containerHome, HOME_LABELS, joinLabel } from "./container";
import { allowListAll, allowMachine, grantsFor, homeMentionsArg, resetGrants } from "./grants";
import { parseMentions, saidWhere } from "./mentions";
import { abilityOfAction, approvalAction, homePrompt } from "./prompt";
import { askText, covered, openAbilityRequests } from "./requests";
import { retryTarget } from "@/lib/ask-first";
import { forgetHome, homeSpaceId, isHomeSpaceId, homeForRequest, rememberHome, resetHomes, reusableHome } from "./home-space";

beforeEach(() => {
  localStorage.clear();
  resetGrants();
  resetHomes();
});

const place = {
  spaces: [
    { id: "local:ci-linux", name: "ci-linux" },
    { id: "local:studio", name: "studio" },
  ],
  machines: [
    { id: "m-air", name: "Dillon's MacBook Air", current: true },
    { id: "m-mini", name: "spare mac mini", current: false },
  ],
};

describe("where the person said the job runs", () => {
  it("reads @mentions of Spaces and machines, by name or first word", () => {
    const m = parseMentions('Run the tests on @ci-linux and @"spare mac mini", then @Dillon', place);
    expect(m.spaces).toEqual([{ id: "local:ci-linux", name: "ci-linux" }]);
    expect(m.machines.map((x) => x.id).sort()).toEqual(["m-air", "m-mini"]);
    expect(saidWhere(m)).toBe(true);
  });

  it('reads "on my Mac" as this machine and "connect to <x>" as that one', () => {
    expect(parseMentions("build it on my Mac", place).machines.map((m) => m.id)).toEqual(["m-air"]);
    expect(parseMentions("connect to spare mac mini and look", place).machines.map((m) => m.id)).toEqual(["m-mini"]);
    expect(parseMentions("connect to studio", place).spaces.map((s) => s.id)).toEqual(["local:studio"]);
  });

  it("leaves the choice to the agent when nothing is named", () => {
    const m = parseMentions("test https://github.com/acme/app", place);
    expect(m).toEqual(NO_MENTIONS);
    expect(saidWhere(m)).toBe(false);
  });
});

describe("what the person approved is remembered per agent", () => {
  it("starts with nothing and keeps each agent's grants apart", () => {
    expect(grantsFor("claude-code")).toEqual(NO_GRANTS);
    allowListAll("claude-code");
    allowMachine("claude-code", "m-air");
    allowMachine("openai-codex", "m-mini");
    resetGrants();
    expect(grantsFor("claude-code")).toEqual({ listAll: true, machines: ["m-air"] });
    expect(grantsFor("openai-codex")).toEqual({ listAll: false, machines: ["m-mini"] });
    allowMachine("claude-code", "m-air", false);
    expect(grantsFor("claude-code").machines).toEqual([]);
  });
});

describe("the container home", () => {
  it("is the slim Linux image with the spawn capability on", () => {
    const c = containerHome.create({ agent: "claude-code", grants: NO_GRANTS, mentions: NO_MENTIONS }, "home-x");
    expect(c).toMatchObject({ image: AGENT_HOME_IMAGE, kind: "container", on: "local", children: true, spacesd: true });
    expect(c.labels).toEqual({ [HOME_LABELS.home]: "1", [HOME_LABELS.agent]: "claude-code" });
  });

  it("carries what was granted and named as labels the daemon reads", () => {
    const c = containerHome.create(
      {
        agent: "claude-code",
        grants: { listAll: true, machines: ["m-air"] },
        mentions: { spaces: [{ id: "local:studio", name: "studio" }], machines: [{ id: "m-mini", name: "spare mac mini" }] },
      },
      "home-x",
    );
    expect(c.labels[HOME_LABELS.list]).toBe("all");
    expect(c.labels[HOME_LABELS.mentions]).toBe("local:studio,m-mini,spare mac mini,studio");
    expect(c.labels[HOME_LABELS.machines]).toBe("m-air,m-mini,spare mac mini");
  });

  it("keeps a label value within the daemon's 256 bytes", () => {
    const many = Array.from({ length: 80 }, (_, i) => `space-number-${i}`);
    expect(new TextEncoder().encode(joinLabel(many)).length).toBeLessThanOrEqual(256);
  });

  it("offers the three abilities the agent is told about", () => {
    expect(containerHome.abilities().map((a) => a.id)).toEqual([...ABILITY_IDS]);
  });

  it("uses the labels the daemon reads (children/home.rs)", () => {
    const rs = readFileSync(resolve(__dirname, "../../../../../libs/cua/crates/cua-daemon/src/children/home.rs"), "utf8");
    for (const label of Object.values(HOME_LABELS)) expect(rs).toContain(`"${label}"`);
  });

  it("runs on the slim Linux image of the catalog", () => {
    const root = resolve(__dirname, "../../../../../");
    const catalog = JSON.parse(readFileSync(resolve(root, "libs/images/sandbox-images.json"), "utf8")) as {
      images: { ref: string; variant: string; os: string; spacesd: boolean; tier?: string }[];
    };
    const entry = catalog.images.find((i) => i.ref === AGENT_HOME_IMAGE);
    expect(entry).toMatchObject({ variant: "container", os: "linux", spacesd: true, tier: "slim" });
  });
});

describe("the agent's prompt", () => {
  const setup = { agent: "claude-code", grants: NO_GRANTS, mentions: NO_MENTIONS };
  it("says where it is, what it can do and how to ask", () => {
    const p = homePrompt("test https://github.com/acme/app", setup, containerHome.abilities());
    expect(p).toContain("your own small Linux Space");
    expect(p).toContain("posted to the thread as its own message");
    expect(p).toContain("cua spaces ls");
    // GPU work goes to the person's computers through the pool.
    expect(p).toContain("@app.function(gpu=");
    expect(p).toContain("cua run file.py::fn");
    // What it needs is in the brief: nothing sends it to read a skill in the thread.
    expect(p).not.toMatch(/read the .* skill/i);
    expect(p).toContain(approvalAction("machines.use"));
    // A window the agent opened in a child is shown live in the home's Stream list, not sent as a screenshot.
    expect(p).toContain("stream_space_window");
    expect(p).toContain("never delete a Space the person asked to see");
    expect(p).toContain("Unless the person says where");
    expect(p.endsWith("test https://github.com/acme/app")).toBe(true);
  });
  it("makes the agent speak as the person's supercomputer, not narrate its process", () => {
    const p = homePrompt("train a model", setup, containerHome.abilities());
    expect(p).toContain("You are the person's supercomputer");
    expect(p).toContain("one short final message");
    expect(p).toContain("no narration of steps, tools, skills, files, commands, plans or checks");
    expect(p).toContain("Do not ask the person which computer or tool to use");
    expect(p).toContain("stream_space_window");
  });
  it("treats a named Space or machine as the grant", () => {
    const p = homePrompt("x", { ...setup, mentions: { spaces: [], machines: [{ id: "m-air", name: "my Air" }] } }, containerHome.abilities());
    expect(p).toContain('the computer "my Air"');
    expect(p).toContain("without asking");
  });
  it("round-trips an ability's action word", () => {
    for (const id of ABILITY_IDS) expect(abilityOfAction(approvalAction(id))).toBe(id);
    expect(abilityOfAction("Push to main")).toBeNull();
  });
});

describe("the agent's requests", () => {
  const entry = (seq: number, e: Partial<ThreadEntry>): ThreadEntry => ({
    seq,
    id: String(seq),
    tsMs: seq,
    kind: "message",
    text: "",
    mentions: [],
    replyTo: null,
    task: null,
    attachments: [],
    approval: null,
    answer: null,
    noteFor: null,
    update: null,
    runsOnKeysOf: null,
    event: null,
    author: { kind: "agent", id: "agent:claude", name: "claude" },
    ...e,
  });
  const ask = (seq: number, id: string, details = "") =>
    entry(seq, { kind: "approval", approval: { action: approvalAction(id as never), details, state: "open" } });

  it("lists the open ones and skips answered and other approvals", () => {
    const entries = [
      ask(1, "spaces.list"),
      ask(2, "machines.use", "the spare mac mini"),
      entry(3, { kind: "answer", answer: { approval: 1, approve: true, note: null } }),
      entry(4, { kind: "approval", approval: { action: "Push to main", details: "", state: "open" } }),
    ];
    expect(openAbilityRequests(entries).map((r) => [r.seq, r.ability])).toEqual([[2, "machines.use"]]);
  });

  it("is covered by a remembered grant, and spawning never asks", () => {
    const [list, mac, spawn] = openAbilityRequests([ask(1, "spaces.list"), ask(2, "machines.use", "use the spare mac mini"), ask(3, "spaces.spawn")]);
    const names = new Map([["m-mini", "spare mac mini"]]);
    expect(covered(list!, NO_GRANTS, names)).toBe(false);
    expect(covered(list!, { listAll: true, machines: [] }, names)).toBe(true);
    expect(covered(mac!, NO_GRANTS, names)).toBe(false);
    expect(covered(mac!, { listAll: false, machines: ["m-mini"] }, names)).toBe(true);
    expect(covered(spawn!, NO_GRANTS, names)).toBe(true);
  });

  it("asks in plain words, never with a middle dot", () => {
    const [list, mac] = openAbilityRequests([ask(1, "spaces.list"), ask(2, "machines.use", "build the app")]);
    expect(askText(list!, null).title).toBe("Let this agent see all your Spaces?");
    expect(askText(mac!, "this Mac").title).toBe("Let this agent use this Mac?");
    for (const t of [askText(list!, null), askText(mac!, "this Mac")]) expect(`${t.title}${t.detail}${t.allow}`).not.toContain("·");
  });
});

describe("an @mention to a home that already runs is the grant", () => {
  it("sends every id and name of what was named, so the daemon matches either, and nothing else", () => {
    const named = parseMentions("check @studio and use @my-mac", {
      spaces: [{ id: "local:studio", name: "studio" }],
      machines: [{ id: "m-air", name: "my-mac", current: false }],
    });
    expect(homeMentionsArg(named)).toEqual(["local:studio", "m-air", "my-mac", "studio"]);
    expect(homeMentionsArg(NO_MENTIONS)).toEqual([]);
    // It never turns on listing everything.
    expect(grantsFor("claude-code").listAll).toBe(false);
  });
});

describe("the home Space is made once and reused", () => {
  const spaces = [
    { id: "local:home-a", status: "running" as const },
    { id: "local:other", status: "running" as const },
  ];

  it("remembers one home per agent and tells the list which Space it is", () => {
    expect(homeSpaceId("claude-code")).toBeNull();
    rememberHome("claude-code", "local:home-a");
    rememberHome("openai-codex", "local:home-b");
    resetHomes();
    expect(homeSpaceId("claude-code")).toBe("local:home-a");
    expect(homeSpaceId("openai-codex")).toBe("local:home-b");
    expect(isHomeSpaceId("local:home-a")).toBe(true);
    expect(isHomeSpaceId("local:other")).toBe(false);
    forgetHome("claude-code");
    expect(isHomeSpaceId("local:home-a")).toBe(false);
  });

  it("reuses the home while it is listed, and starts a new one when asked or when it is gone", () => {
    rememberHome("claude-code", "local:home-a");
    expect(reusableHome("claude-code", spaces)).toBe("local:home-a");
    expect(reusableHome("claude-code", spaces, true)).toBeNull();
    expect(reusableHome("claude-code", [spaces[1]!])).toBeNull();
    expect(reusableHome("claude-code", [{ id: "local:home-a", status: "deleting" }])).toBeNull();
    expect(reusableHome("openai-codex", spaces)).toBeNull();
  });

  it("finds the home again by its label when this device remembers nothing", () => {
    // A new device, or the app's storage was cleared: the host says which Spaces are homes.
    expect(homeSpaceId("claude-code")).toBeNull();
    expect(reusableHome("claude-code", spaces, false, ["local:home-a"])).toBe("local:home-a");
    // Gone or being deleted: not offered; the next labelled one is.
    expect(reusableHome("claude-code", [spaces[1]!], false, ["local:home-a"])).toBeNull();
    expect(reusableHome("claude-code", [{ id: "local:home-a", status: "deleting" }, spaces[1]!], false, ["local:home-a", "local:other"])).toBe("local:other");
    // A fresh home is asked for: the label does not bring the old one back.
    expect(reusableHome("claude-code", spaces, true, ["local:home-a"])).toBeNull();
    // What this device remembers still wins over the labels.
    rememberHome("claude-code", "local:other");
    expect(reusableHome("claude-code", spaces, false, ["local:home-a"])).toBe("local:other");
  });

  it("a message from Home makes a new home every time; only picking the existing home continues it", () => {
    rememberHome("claude-code", "local:home-a");
    // First and second Home messages: neither reuses, so each makes its own home.
    expect(homeForRequest({ kind: "agent" }, "claude-code", spaces, ["local:home-a"])).toBeNull();
    expect(homeForRequest({ kind: "machine" }, "claude-code", spaces, ["local:home-a"])).toBeNull();
    // The second home is remembered as the newest; the first stays in the list.
    rememberHome("claude-code", "local:home-b");
    expect(homeSpaceId("claude-code")).toBe("local:home-b");
    expect(isHomeSpaceId("local:home-a")).toBe(false);
    // Explicitly continuing the existing home reuses it (the newest, found again by label on a new device).
    const both = [
      { id: "local:home-a", status: "running" as const },
      { id: "local:home-b", status: "running" as const },
    ];
    expect(homeForRequest({ kind: "agent", reuse: true }, "claude-code", both, ["local:home-a"])).toBe("local:home-b");
    // A home that is gone is not reused even when asked.
    expect(homeForRequest({ kind: "agent", reuse: true }, "claude-code", [], [])).toBeNull();
    // A picked Space is not a home request at all.
    expect(homeForRequest({ kind: "space" }, "claude-code", both)).toBeNull();
  });

  it("retry keeps the Space the task was made in (a new home only when none was made yet)", () => {
    expect(retryTarget({ spaceId: "local:home-a", os: "linux", onHost: false })).toEqual({ kind: "space", spaceId: "local:home-a" });
    expect(retryTarget({ spaceId: null, os: "linux", onHost: false })).toEqual({ kind: "create", os: "linux" });
  });
});

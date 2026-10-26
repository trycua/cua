// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { activeMention, insertMention, matchMentions, mentionSegments, type MentionCandidate } from "./mentions";

const PEOPLE: MentionCandidate[] = [
  { handle: "grace", label: "Grace", kind: "person" },
  { handle: "claude", label: "Claude Code", kind: "agent" },
  { handle: "hermes", label: "Hermes", kind: "agent" },
];

describe("the composer's mentions", () => {
  it("finds the mention at the caret, never inside a word", () => {
    expect(activeMention("@", 1)).toEqual({ start: 0, query: "" });
    expect(activeMention("hi @cl", 6)).toEqual({ start: 3, query: "cl" });
    expect(activeMention("(@her", 5)).toEqual({ start: 1, query: "her" });
    expect(activeMention("ada@example.com", 15)).toBeNull();
    expect(activeMention("@claude run it", 14)).toBeNull();
    expect(activeMention("no at sign", 4)).toBeNull();
  });

  it("matches handles and words of labels, agents first", () => {
    expect(matchMentions("", PEOPLE).map((c) => c.handle)).toEqual(["claude", "hermes", "grace"]);
    expect(matchMentions("co", PEOPLE).map((c) => c.handle)).toEqual(["claude"]);
    expect(matchMentions("G", PEOPLE).map((c) => c.handle)).toEqual(["grace"]);
    expect(matchMentions("zz", PEOPLE)).toEqual([]);
  });

  it("puts the picked name in place of what was typed", () => {
    expect(insertMention("@cl", 0, 3, "claude")).toEqual({ text: "@claude ", caret: 8 });
    expect(insertMention("ask @he now", 4, 7, "hermes")).toEqual({ text: "ask @hermes now", caret: 11 });
    expect(insertMention("@clau", 0, 3, "claude")).toEqual({ text: "@claude ", caret: 8 });
  });

  it("splits text into mentions of members and the rest", () => {
    const handles = new Set(["claude", "hermes"]);
    expect(mentionSegments("@claude run it, @nobody too. thanks @hermes.", handles)).toEqual([
      { text: "@claude", mention: true, handle: "claude" },
      { text: " run it, @nobody too. thanks ", mention: false },
      { text: "@hermes", mention: true, handle: "hermes" },
      { text: ".", mention: false },
    ]);
    expect(mentionSegments("mail ada@example.com")).toEqual([{ text: "mail ada@example.com", mention: false }]);
  });
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { activeMention, addLeadingMention, insertMention, matchMentions, mentionedHandles, mentionSegments, startsWithMention, type MentionCandidate } from "./mentions";

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

describe("addressing from the text", () => {
  it("lists the handles a text mentions, once each, lower case", () => {
    expect(mentionedHandles("@Claude and @hermes, then @claude again")).toEqual(["claude", "hermes"]);
    expect(mentionedHandles("mail ada@example.com")).toEqual([]);
    expect(mentionedHandles("@claude @nobody", new Set(["claude"]))).toEqual(["claude"]);
  });

  it("knows whether a text already starts with a mention of a handle", () => {
    expect(startsWithMention("@claude go", "claude")).toBe(true);
    expect(startsWithMention("  @Claude. go", "claude")).toBe(true);
    expect(startsWithMention("ask @claude", "claude")).toBe(false);
    expect(startsWithMention("@hermes go", "claude")).toBe(false);
  });

  it("puts the missing mentions in front, once", () => {
    expect(addLeadingMention("run the tests", "claude")).toBe("@claude run the tests");
    expect(addLeadingMention("run the tests", "claude", "hermes")).toBe("@claude @hermes run the tests");
    expect(addLeadingMention("@claude run the tests", "claude")).toBe("@claude run the tests");
    expect(addLeadingMention("ask @claude to run", "claude")).toBe("ask @claude to run");
    expect(addLeadingMention("@claude go", "claude", "hermes")).toBe("@hermes @claude go");
  });
});

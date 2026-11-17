// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { parseBlocks, parseInline } from "./markdown";

describe("parseBlocks", () => {
  it("reads the blocks agents write", () => {
    const blocks = parseBlocks(
      "## Plan\n\nFirst a paragraph\nthat wraps.\n\n1. one\n2. two\n\n- a\n- b\n\n> quoted\n\n```ts\nconst a = 1;\n```\n\n---\nEnd.",
    ).map((b) => b.block);
    expect(blocks).toEqual([
      { type: "heading", level: 2, text: "Plan" },
      { type: "paragraph", text: "First a paragraph\nthat wraps." },
      { type: "list", ordered: true, start: 1, items: ["one", "two"] },
      { type: "list", ordered: false, start: 1, items: ["a", "b"] },
      { type: "quote", text: "quoted" },
      { type: "code", lang: "ts", code: "const a = 1;", open: false },
      { type: "rule" },
      { type: "paragraph", text: "End." },
    ]);
  });

  it("keeps a fence that is still streaming open, and earlier blocks unchanged", () => {
    const done = parseBlocks("Intro.\n\n```diff\n- old\n+ new\n```");
    const streaming = parseBlocks("Intro.\n\n```diff\n- old\n+ ne");
    expect(streaming[0]!.source).toBe(done[0]!.source);
    expect(streaming[1]!.block).toEqual({ type: "code", lang: "diff", code: "- old\n+ ne", open: true });
  });

  it("ends a paragraph where a list starts", () => {
    expect(parseBlocks("Two things:\n- a\n- b").map((b) => b.block.type)).toEqual(["paragraph", "list"]);
  });
});

describe("parseInline", () => {
  it("reads code, bold, italics and links", () => {
    expect(parseInline("Run `pnpm test`, **then** _commit_ ([docs](https://cua.ai/docs)).")).toEqual([
      { type: "text", text: "Run " },
      { type: "code", text: "pnpm test" },
      { type: "text", text: ", " },
      { type: "strong", children: [{ type: "text", text: "then" }] },
      { type: "text", text: " " },
      { type: "em", children: [{ type: "text", text: "commit" }] },
      { type: "text", text: " (" },
      { type: "link", href: "https://cua.ai/docs", children: [{ type: "text", text: "docs" }] },
      { type: "text", text: ")." },
    ]);
  });

  it("leaves unclosed marks, snake_case and unsafe links as text", () => {
    expect(parseInline("half **bold")).toEqual([{ type: "text", text: "half **bold" }]);
    expect(parseInline("a snake_case_name")).toEqual([{ type: "text", text: "a snake_case_name" }]);
    expect(parseInline("[x](javascript:alert(1))")).toEqual([{ type: "text", text: "[x](javascript:alert(1))" }]);
  });

  it("links bare URLs without trailing punctuation", () => {
    expect(parseInline("See https://cua.ai/docs.")).toEqual([
      { type: "text", text: "See " },
      { type: "link", href: "https://cua.ai/docs", children: [{ type: "text", text: "https://cua.ai/docs" }] },
      { type: "text", text: "." },
    ]);
  });
});

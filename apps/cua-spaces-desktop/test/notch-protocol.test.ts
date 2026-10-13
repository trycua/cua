// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { enumName, needsRow, wireLayout, wireMotion, wireView } from "../src/notch/core";
import { encodeMessage, LineSplitter, MAX_LINE, parseHelperMessage, type HostMessage, type StateMessage } from "../src/notch/protocol";

// The same lines the Swift helper's tests decode and encode
// (libs/spaces-notch-swift/Tests/CuaSpacesNotchTests/Fixtures).
const fixtures = path.join(__dirname, "../../../libs/spaces-notch-swift/Tests/CuaSpacesNotchTests/Fixtures");
const lines = (name: string) => readFileSync(path.join(fixtures, name), "utf8").split("\n").filter(Boolean);
const hostFixture = () => lines("host-messages.jsonl").map((l) => JSON.parse(l) as HostMessage);

/** The fixture state's view as the cua-spaces-ffi Node bindings return it:
 * flat enums as numbers, u64/i64 as bigint, absent optionals undefined. */
function bindingView() {
  return {
    phase: 1,
    tiles: [
      {
        id: "local:aurora",
        name: "aurora",
        os: 0,
        status: 1,
        dim: false,
        dropTarget: true,
        targeted: true,
        symbol: "apple",
        label: "aurora, running on This Mac",
        location: "This Mac",
        progress: undefined,
        progressLabel: undefined,
        signedIn: true,
      },
      {
        id: "cloud:build",
        name: "build",
        os: 1,
        status: 4,
        dim: true,
        dropTarget: false,
        targeted: false,
        symbol: "ubuntu",
        label: "build, starting",
        location: "Cua Cloud",
        progress: 420,
        progressLabel: "Starting",
        signedIn: false,
      },
    ],
    dropMode: true,
    prompt: "Drop on a Space",
    label: "Cua Spaces, 2 Spaces",
    countLabel: "2 Spaces",
    tab: { count: "2", word: "Spaces" },
    header: {
      query: "au",
      placeholder: "Search Spaces",
      searchLabel: "Search Spaces",
      matchCount: 1,
      buttons: [
        { id: 0, symbol: "list.bullet", label: "Spaces", help: "Show all Spaces" },
        { id: 1, symbol: "gearshape", label: "Settings", help: "Settings" },
      ],
    },
    empty: undefined,
    activity: { kind: 0, label: "Sending 1 file", symbol: undefined, permille: 600, startedAt: 1767225600000n, estimateMs: 8000 },
    hidden: false,
    showTab: true,
    hoverCue: false,
    permission: { text: "Allow Accessibility to drag windows here", action: "Open Settings", pane: "accessibility" },
    access: { text: "github.com live in aurora", dismiss: "Dismiss" },
  };
}

describe("notch protocol", () => {
  it("turns the core's view into exactly what the helper decodes", () => {
    const state = hostFixture()[1] as StateMessage;
    expect(JSON.parse(JSON.stringify(wireView(bindingView())))).toEqual(state.view);
    expect(wireLayout(state.layout)).toEqual(state.layout);
    expect(needsRow(state.view)).toBe(true);
  });

  it("encodes the core's motion as numbers", () => {
    const hello = hostFixture()[0] as Extract<HostMessage, { type: "hello" }>;
    const binding = { ...hello.motion, hoverDwellMs: 300, closeDelayMs: 400, contentDelayMs: 90 };
    expect(wireMotion(binding)).toEqual(hello.motion);
  });

  it("encodes each message as one line the helper reads", () => {
    for (const m of hostFixture()) {
      const line = encodeMessage(m);
      expect(line.endsWith("\n")).toBe(true);
      expect(line.slice(0, -1)).not.toContain("\n");
      expect(JSON.parse(line)).toEqual(m);
    }
    // Absent optionals are left out, as Swift's decoder expects.
    expect(encodeMessage({ type: "ghost", image: undefined })).toBe('{"type":"ghost"}\n');
  });

  it("reads every message the helper writes", () => {
    for (const line of lines("helper-messages.jsonl")) {
      const parsed = parseHelperMessage(line);
      expect(parsed, line).not.toBeNull();
      expect(parsed).toEqual(JSON.parse(line));
    }
  });

  it("refuses malformed or unknown messages", () => {
    for (const line of [
      "not json",
      "[]",
      '{"type":"teleport"}',
      '{"type":"hello","v":"1"}',
      '{"type":"event","event":{"kind":"wiggle"}}',
      '{"type":"event","event":{"kind":"search"}}',
      '{"type":"action","action":"openSpace"}',
      '{"type":"action","action":"rm -rf"}',
      '{"type":"action","action":"drop","spaceId":"a","paths":[1]}',
      '{"type":"screens","notch":{"frame":{}}}',
      '{"type":"stage","open":"yes"}',
    ]) {
      expect(parseHelperMessage(line), line).toBeNull();
    }
  });

  it("splits a stream into lines across chunks and drops an oversized line whole", () => {
    const s = new LineSplitter();
    expect(s.push('{"type":"st')).toEqual([]);
    expect(s.push('age","open":true}\n\n{"type"')).toEqual(['{"type":"stage","open":true}']);
    expect(s.push(':"hello","v":1,"pid":2}\n')).toEqual(['{"type":"hello","v":1,"pid":2}']);
    expect(s.push("x".repeat(MAX_LINE + 1))).toEqual([]);
    expect(s.push('still the big one\n{"type":"stage","open":false}\n')).toEqual(['{"type":"stage","open":false}']);
  });

  it("names flat enums by the bindings' numbers or by name", () => {
    const phases = ["closed", "tiles", "prompt"] as const;
    expect(enumName(phases, 2)).toBe("prompt");
    expect(enumName(phases, "Tiles")).toBe("tiles");
    expect(() => enumName(phases, 3)).toThrow(/unknown enum value/);
  });
});

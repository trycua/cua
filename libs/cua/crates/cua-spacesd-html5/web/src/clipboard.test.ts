// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { isEmpty, sameContent } from "./clipboard";
import { floatToS16le, resample } from "./mic";

describe("clipboard content", () => {
  it("compares text and image flavors", () => {
    expect(sameContent({ text: "a" }, { text: "a" })).toBe(true);
    expect(sameContent({ text: "a" }, { text: "b" })).toBe(false);
    expect(sameContent({ png: new Uint8Array([1, 2]) }, { png: new Uint8Array([1, 2]) })).toBe(true);
    expect(sameContent({ png: new Uint8Array([1, 2]) }, { png: new Uint8Array([1, 3]) })).toBe(false);
    expect(sameContent({ text: "a", png: new Uint8Array([1]) }, { text: "a" })).toBe(false);
    expect(sameContent(null, null)).toBe(true);
    expect(isEmpty({})).toBe(true);
  });
});

describe("mic PCM", () => {
  it("interleaves planes as s16le", () => {
    const bytes = floatToS16le([new Float32Array([1, -1]), new Float32Array([0, 0.5])]);
    const v = new DataView(bytes.buffer);
    expect([v.getInt16(0, true), v.getInt16(2, true), v.getInt16(4, true), v.getInt16(6, true)]).toEqual([32767, 0, -32768, 16383]);
  });
});

describe("mic resampling", () => {
  it("keeps length ratios and endpoints", () => {
    const x = new Float32Array([0, 1, 0, -1]);
    expect(resample(x, 48000, 48000)).toBe(x);
    const up = resample(x, 44100, 48000);
    expect(up.length).toBe(Math.round((4 * 48000) / 44100));
    expect(up[0]).toBe(0);
  });
});

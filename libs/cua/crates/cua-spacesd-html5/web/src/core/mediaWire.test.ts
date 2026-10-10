// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  AUDIO_HEADER_BYTES,
  classifyAudioSequence,
  closeAction,
  coalesceText,
  decodeControl,
  encodeAudioPacket,
  encodeControl,
  encodeVideoPacket,
  InputSequencer,
  JITTER_MAX_MS,
  JITTER_MIN_MS,
  JitterController,
  keyEventToInput,
  normalizePoint,
  parseAudioHeader,
  parseBinary,
  pointerButton,
  serialDiff32,
  ticketValid,
  type VideoDescriptor,
} from "./mediaWire";

const DESCRIPTOR: VideoDescriptor = {
  session_id: "m-1",
  sequence: 4_000_000_123,
  geometry_epoch: 3,
  codec_epoch: 2,
  width_px: 2,
  height_px: 1,
  capture_timestamp_us: 99,
  codec: "bgra",
  keyframe: true,
};

describe("binary demux (MEDIA.md §12.2)", () => {
  it("parses an RAU2 audio header field by field", () => {
    const buffer = encodeAudioPacket(
      { trackId: 7, sequence: 0xffff_fffe, ptsUs: 2 ** 40 + 5, frameSamples: 960, configEpoch: 3, flags: 0b11 },
      new Uint8Array([1, 2, 3]),
    );
    const header = parseAudioHeader(buffer)!;
    expect(header).toMatchObject({
      trackId: 7,
      sequence: 0xffff_fffe,
      ptsUs: 2 ** 40 + 5,
      frameSamples: 960,
      configEpoch: 3,
      discontinuity: true,
      dtx: true,
    });
    const packet = parseBinary(buffer);
    expect(packet.kind).toBe("audio");
    if (packet.kind === "audio") expect([...packet.payload]).toEqual([1, 2, 3]);
  });

  it("drops audio with a bad version, reserved bits, track 0 or a short header", () => {
    const good = encodeAudioPacket({ trackId: 1, sequence: 1, ptsUs: 0, frameSamples: 960, configEpoch: 0 }, new Uint8Array(4));
    const bad = (mutate: (v: DataView) => void) => {
      const copy = good.slice(0);
      mutate(new DataView(copy));
      return parseAudioHeader(copy);
    };
    expect(bad((v) => v.setUint8(4, 1))).toBeNull();
    expect(bad((v) => v.setUint8(5, 0b100))).toBeNull();
    expect(bad((v) => v.setUint8(23, 1))).toBeNull();
    expect(bad((v) => v.setUint16(6, 0))).toBeNull();
    expect(parseAudioHeader(good.slice(0, AUDIO_HEADER_BYTES - 1))).toBeNull();
    expect(parseBinary(good.slice(0, 10)).kind).toBe("invalid");
  });

  it("tells length-prefixed video (first byte 0x00) from audio (0x52)", () => {
    const buffer = encodeVideoPacket(DESCRIPTOR, new Uint8Array([9, 8, 7, 6, 5, 4, 3, 2]));
    expect(new Uint8Array(buffer)[0]).toBe(0);
    const packet = parseBinary(buffer);
    expect(packet.kind).toBe("video");
    if (packet.kind === "video") {
      expect(packet.descriptor.sequence).toBe(4_000_000_123);
      expect(packet.payload.byteLength).toBe(8);
    }
  });

  it("rejects a video packet whose lengths do not add up", () => {
    const buffer = encodeVideoPacket(DESCRIPTOR, new Uint8Array(4));
    expect(parseBinary(buffer.slice(0, buffer.byteLength - 1)).kind).toBe("invalid");
  });
});

describe("control frames", () => {
  it("decodes the bare v2 shape and the v1 server envelope", () => {
    expect(decodeControl('{"type":"hello","payload":{"selected_version":2}}')).toEqual({
      type: "hello",
      payload: { selected_version: 2 },
    });
    expect(decodeControl('{"direction":"server","message":{"type":"pong","payload":{"nonce":1}}}')?.type).toBe("pong");
    expect(decodeControl('{"type":"resumed"}')).toEqual({ type: "resumed", payload: {} });
  });

  it("ignores client-direction envelopes and garbage", () => {
    expect(decodeControl('{"direction":"client","message":{"type":"ping"}}')).toBeNull();
    expect(decodeControl("not json")).toBeNull();
    expect(decodeControl("[1,2]")).toBeNull();
  });

  it("encodes client frames bare", () => {
    expect(JSON.parse(encodeControl("request_keyframe", { session_id: "m" }))).toEqual({
      type: "request_keyframe",
      payload: { session_id: "m" },
    });
  });
});

describe("audio sequences (RFC 1982, §12.4)", () => {
  it("computes signed serial distance across the u32 wrap", () => {
    expect(serialDiff32(0xffff_ffff, 0)).toBe(1);
    expect(serialDiff32(0, 0xffff_ffff)).toBe(-1);
    expect(serialDiff32(10, 15)).toBe(5);
  });

  it("classifies next / gap / late", () => {
    expect(classifyAudioSequence(null, 42).kind).toBe("first");
    expect(classifyAudioSequence(41, 42).kind).toBe("next");
    expect(classifyAudioSequence(0xffff_ffff, 1)).toEqual({ kind: "gap", lost: 1 });
    expect(classifyAudioSequence(42, 42).kind).toBe("late");
    expect(classifyAudioSequence(42, 40).kind).toBe("late");
  });
});

describe("InputSequencer (§6)", () => {
  it("starts anywhere and hands out contiguous ranges", () => {
    const s = new InputSequencer(1_000);
    expect(s.take(3)).toBe(1_000);
    expect(s.take(2)).toBe(1_003);
    expect(s.peek()).toBe(1_005);
  });

  it("re-syncs to expected_sequence after input_sequence_gap and ignores junk", () => {
    const s = new InputSequencer(5);
    s.take(10);
    s.resync(7);
    expect(s.take(1)).toBe(7);
    s.resync(undefined);
    s.resync(Number.NaN);
    expect(s.peek()).toBe(8);
  });

  it("defaults to a random non-zero base", () => {
    expect(new InputSequencer().peek()).toBeGreaterThan(0);
  });
});

describe("close codes (§10) and tickets", () => {
  it("maps close codes onto reattach / reopen / gone", () => {
    expect(closeAction(1000, true)).toBe("stop");
    expect(closeAction(4401, true)).toBe("reopen");
    expect(closeAction(4404, true)).toBe("gone");
    expect(closeAction(4410, true)).toBe("gone");
    expect(closeAction(1006, true)).toBe("reattach");
    expect(closeAction(1006, false)).toBe("reopen");
    expect(closeAction(4408, true)).toBe("reattach");
  });

  it("treats a ticket as valid until shortly before expiry", () => {
    const now = Date.parse("2026-09-22T12:00:00Z");
    expect(ticketValid(undefined, now)).toBe(true);
    expect(ticketValid("2026-09-22T12:01:00Z", now)).toBe(true);
    expect(ticketValid("2026-09-22T12:00:01Z", now)).toBe(false);
    expect(ticketValid("garbage", now)).toBe(true);
  });
});

describe("JitterController (§12.5)", () => {
  it("starts at 2 × frame_ms clamped to 20–60 ms", () => {
    expect(new JitterController(20).target).toBe(40);
    expect(new JitterController(5).target).toBe(JITTER_MIN_MS);
    expect(new JitterController(60).target).toBe(JITTER_MAX_MS);
  });

  it("grows at once on underrun, up to the ceiling", () => {
    const j = new JitterController(20, 100);
    j.onUnderrun();
    expect(j.target).toBe(60);
    for (let i = 0; i < 10; i++) j.onUnderrun();
    expect(j.target).toBe(100);
  });

  it("follows interarrival jitter and shrinks by at most 5 ms/s while silent", () => {
    const j = new JitterController(20);
    // Packets every 20 ms of media time arriving with ±15 ms wobble.
    for (let i = 0; i < 64; i++) j.onPacket(i * 20_000, i * 20 + (i % 2 ? 15 : 0));
    const grown = j.target;
    expect(grown).toBeGreaterThan(40);
    j.onSilence(0);
    j.onSilence(1_000);
    expect(j.target).toBeGreaterThanOrEqual(grown - 5.0001);
    expect(j.target).toBeLessThanOrEqual(grown);
  });
});

describe("input mapping", () => {
  it("commits printable keys as text on keydown only", () => {
    const base = { shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, repeat: false };
    expect(keyEventToInput({ ...base, key: "a" }, "down")).toEqual({ kind: "text_commit", text: "a" });
    expect(keyEventToInput({ ...base, key: "a" }, "up")).toBeNull();
    expect(keyEventToInput({ ...base, key: "Enter" }, "down")).toEqual({
      kind: "key",
      key: "enter",
      state: "down",
      modifiers: [],
      repeat: false,
    });
    expect(keyEventToInput({ ...base, key: "c", metaKey: true }, "down")).toMatchObject({
      kind: "key",
      key: "c",
      modifiers: ["command"],
    });
  });

  it("coalesces adjacent text commits without crossing other events", () => {
    const out = coalesceText([
      { kind: "text_commit", text: "h" },
      { kind: "text_commit", text: "i" },
      { kind: "key", key: "enter", state: "down", modifiers: [], repeat: false },
      { kind: "text_commit", text: "!" },
    ]);
    expect(out.map((e) => e.kind)).toEqual(["text_commit", "key", "text_commit"]);
    expect(out[0]).toEqual({ kind: "text_commit", text: "hi" });
  });

  it("normalizes and clamps pointer positions; maps buttons", () => {
    const rect = { left: 10, top: 20, width: 100, height: 50 };
    expect(normalizePoint(60, 45, rect)).toEqual({ x_normalized: 0.5, y_normalized: 0.5 });
    expect(normalizePoint(-5, 500, rect)).toEqual({ x_normalized: 0, y_normalized: 1 });
    expect(pointerButton(0)).toBe("left");
    expect(pointerButton(2)).toBe("right");
    expect(pointerButton(9)).toBe("left");
  });
});

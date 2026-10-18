// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest"
import { avcCodecString, nalUnits, parsePacket, videoOf } from "./rcdp"

function packet(header: object, payload: Uint8Array): ArrayBuffer {
  const h = new TextEncoder().encode(JSON.stringify(header))
  const buf = new ArrayBuffer(8 + h.length + payload.length)
  const v = new DataView(buf)
  v.setUint32(0, h.length, false)
  v.setUint32(4, payload.length, false)
  new Uint8Array(buf, 8).set(h)
  new Uint8Array(buf, 8 + h.length).set(payload)
  return buf
}

// SPS (profile 0x42, constraints 0xC0, level 0x1F), PPS, IDR slice.
const au = new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xaa, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88, 0x84])

describe("rcdp wire v2 video packets", () => {
  it("parses the length-prefixed header and payload", () => {
    const desc = { session_id: "s", sequence: 9, geometry_epoch: 1, codec_epoch: 1, width_px: 800, height_px: 600, capture_timestamp_us: 5, codec: "h264", keyframe: true }
    const p = parsePacket(packet({ direction: "video", message: desc }, au))
    expect(videoOf(p)).toEqual(desc)
    expect(Array.from(p.payload)).toEqual(Array.from(au))
  })
  it("rejects a packet whose lengths do not add up", () => {
    const b = packet({ direction: "video", message: {} }, au)
    expect(() => parsePacket(b.slice(0, b.byteLength - 1))).toThrow(/length/)
    expect(() => parsePacket(new ArrayBuffer(4))).toThrow(/short/)
  })
  it("ignores non-video packets", () => {
    expect(videoOf(parsePacket(packet({ direction: "server", message: { type: "stats" } }, new Uint8Array())))).toBeNull()
  })
})

describe("H.264 Annex B", () => {
  it("splits NAL units on 3- and 4-byte start codes", () => {
    expect(nalUnits(au).map((n) => n[0] & 0x1f)).toEqual([7, 8, 5])
  })
  it("derives the WebCodecs codec string from the SPS", () => {
    expect(avcCodecString(au)).toBe("avc1.42C01F")
    expect(avcCodecString(new Uint8Array([0, 0, 1, 0x65, 1]))).toBeNull()
  })
})

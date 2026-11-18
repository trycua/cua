import assert from "node:assert/strict"
import { test } from "node:test"

import { BGRA, fnv1a64, readTimecode, wavBytes } from "./pixels.ts"

test("fnv1a64 reference vectors", () => {
  assert.equal(fnv1a64(new Uint8Array()), "cbf29ce484222325")
  assert.equal(fnv1a64(new TextEncoder().encode("a")), "af63dc4c8601ec8c")
  assert.equal(fnv1a64(new TextEncoder().encode("foobar")), "85944171f73967e8")
})

function strip(valueMs: number, scale: number): { data: Uint8Array; w: number; h: number } {
  const w = Math.ceil(16 * 48 * scale)
  const h = Math.ceil(16 * scale)
  const data = new Uint8Array(w * h * 4)
  const v = valueMs % 4294967296
  const bits = [1, 0, 1, 0]
  for (let i = 31; i >= 0; i--) bits.push(Math.floor(v / 2 ** i) % 2)
  const x = ((v >>> 24) ^ (v >>> 16) ^ (v >>> 8) ^ v) & 0xff
  for (let i = 7; i >= 0; i--) bits.push((x >> i) & 1)
  bits.push(0, 1, 0, 1)
  for (let y = 0; y < h; y++)
    for (let px = 0; px < w; px++) {
      const c = Math.min(47, Math.floor(px / (16 * scale)))
      data.fill(bits[c] ? 255 : 0, (y * w + px) * 4, (y * w + px) * 4 + 4)
    }
  return { data, w, h }
}

test("timecode round trip, scaled and unscaled", () => {
  const now = 1_790_000_000_123
  for (const scale of [1, 0.75]) {
    const { data, w, h } = strip(now - 40, scale)
    assert.equal(readTimecode(data, w, h, w * 4, scale, BGRA, now), now - 40)
  }
  const { data, w, h } = strip(now, 1)
  for (let y = 0; y < h; y++) data.fill(0, y * w * 4, y * w * 4 + 16 * 4) // cell 0: sync 1 -> 0
  assert.equal(readTimecode(data, w, h, w * 4, 1, BGRA, now), null)
})

test("wav header", () => {
  const wav = wavBytes([new Int16Array([1, -1, 2, -2])], 48000, 2)
  assert.equal(wav.length, 44 + 8)
  assert.equal(new TextDecoder().decode(wav.subarray(0, 4)), "RIFF")
})

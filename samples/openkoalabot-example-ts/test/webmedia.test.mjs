import assert from "node:assert/strict"
import { test } from "node:test"
import { attachMedia, closeMedia, openDesktopMedia, openMediaRequest, parseOpenMedia } from "../dist/core/webmedia.js"
import { encodeVideoPacket } from "../dist/core/wire.js"

test("OpenMedia request and response in proto3 JSON", () => {
  assert.deepEqual(openMediaRequest({ maxFps: 5, maxDimension: 800 }), {
    target: { displayId: "primary" }, codecs: ["MEDIA_CODEC_H264"], maxFps: 5, maxDimension: 800,
  })
  assert.equal(openMediaRequest({ ticketTtlMs: 1500 }).ticketTtl, "1.500s")
  const m = parseOpenMedia("http://127.0.0.1:3211", JSON.stringify({
    mediaSessionId: "m1", ticket: "tk", wsPath: "/media?ticket=tk", codec: "MEDIA_CODEC_H264",
    geometry: { frameSize: { width: 800, height: 500 } },
  }))
  assert.deepEqual(m, { mediaSessionId: "m1", ticket: "tk", wsUrl: "ws://127.0.0.1:3211/media?ticket=tk", codec: "MEDIA_CODEC_H264", width: 800, height: 500 })
  assert.throws(() => parseOpenMedia("http://x", "{}"), /no ticket/)
})

test("open and close go through callJson", async () => {
  const calls = []
  const env = { callJson: async (m, body) => (calls.push([m, JSON.parse(body)]), JSON.stringify({ mediaSessionId: "m", ticket: "t", wsPath: "/media?ticket=t" })) }
  const m = await openDesktopMedia(env, "http://h:1", { maxFps: 2 })
  await closeMedia(env, m.mediaSessionId)
  assert.deepEqual(calls.map((c) => c[0]), ["StreamService/OpenMedia", "StreamService/CloseMedia"])
  assert.deepEqual(calls[1][1], { mediaSessionId: "m" })
})

class FakeWS {
  static last
  constructor(url, protocols) { this.url = url; this.protocols = protocols; this.closed = null; FakeWS.last = this }
  close(code) { this.closed = code }
}

test("attach uses the ticket subprotocol and resolves on the first video packet", async () => {
  const media = { mediaSessionId: "m", ticket: "tk", wsUrl: "ws://h/media?ticket=tk", codec: "", width: 0, height: 0 }
  const seen = []
  const p = attachMedia(FakeWS, media, 1000, (s) => seen.push(s.frames))
  const ws = FakeWS.last
  assert.deepEqual(ws.protocols, ["rcdp.v2", "cua.ticket.tk"])
  ws.onmessage({ data: '{"type":"hello","payload":{}}' })
  ws.onmessage({ data: '{"type":"session_opened","payload":{}}' })
  const pkt = encodeVideoPacket({ sequence: 1, width_px: 2, height_px: 2, codec: "h264", keyframe: true }, new Uint8Array([0, 0, 0, 1, 0x65]))
  ws.onmessage({ data: pkt.buffer })
  const { state } = await p
  assert.equal(state.handshakeOk, true)
  assert.equal(state.firstFrameKeyframe, true)
  assert.deepEqual(seen, [1])
})

test("an unechoed subprotocol falls back to the query ticket once", async () => {
  const media = { mediaSessionId: "m", ticket: "tk", wsUrl: "ws://h/media?ticket=tk", codec: "", width: 0, height: 0 }
  const p = attachMedia(FakeWS, media, 1000)
  const first = FakeWS.last
  first.onclose({ code: 1006, reason: "" })
  await new Promise((r) => setTimeout(r, 0))
  const second = FakeWS.last
  assert.notEqual(second, first)
  assert.equal(second.protocols, undefined)
  const pkt = encodeVideoPacket({ sequence: 1, width_px: 2, height_px: 2, codec: "h264", keyframe: true }, new Uint8Array([1]))
  second.onmessage({ data: pkt.buffer })
  assert.equal((await p).ticketVia, "query")
})

test("attach fails on close or timeout", async () => {
  const media = { mediaSessionId: "m", ticket: "tk", wsUrl: "ws://h/x", codec: "", width: 0, height: 0 }
  const p = attachMedia(FakeWS, media, 1000)
  FakeWS.last.onclose({ code: 4401, reason: "ticket" })
  await assert.rejects(p, /closed 4401/)
  await assert.rejects(attachMedia(FakeWS, media, 20), /no video packet within 20 ms/)
  assert.equal(FakeWS.last.closed, 1000)
})

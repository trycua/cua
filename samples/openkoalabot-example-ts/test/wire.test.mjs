import assert from "node:assert/strict"
import { test } from "node:test"
import {
  MediaSocketState, avcCodecString, decodeControl, encodeControl, encodeVideoPacket, isAnnexBKeyframe,
  mediaSubprotocols, mediaUrl, nalTypes, parseBinary, videoDescriptorOf,
} from "../dist/core/wire.js"

const SPS = [0x67, 0x42, 0xc0, 0x1f, 0xda]
const PPS = [0x68, 0xce, 0x3c, 0x80]
const IDR = [0x65, 0x88, 0x84]
const P = [0x41, 0x9a, 0x02]
const au = (...nals) => new Uint8Array(nals.flatMap((n, i) => [...(i % 2 ? [0, 0, 1] : [0, 0, 0, 1]), ...n]))
const desc = (o = {}) => ({ session_id: "m1", sequence: 7, width_px: 640, height_px: 400, codec: "h264", keyframe: true, ...o })

test("a video packet round-trips through the framing", () => {
  const payload = au(SPS, PPS, IDR)
  const p = parseBinary(encodeVideoPacket(desc(), payload))
  assert.equal(p.kind, "video")
  assert.equal(p.descriptor.sequence, 7)
  assert.equal(p.descriptor.keyframe, true)
  assert.deepEqual([...p.payload], [...payload])
})

test("framing rejects malformed packets without throwing", () => {
  assert.equal(parseBinary(new Uint8Array()).kind, "invalid")
  assert.equal(parseBinary(new Uint8Array([0, 0, 0])).reason, "short packet")
  const good = encodeVideoPacket(desc(), au(SPS, PPS, IDR))
  assert.equal(parseBinary(good.subarray(0, good.length - 1)).reason, "length mismatch")
  const huge = new Uint8Array(8)
  new DataView(huge.buffer).setUint32(0, (1 << 20) + 1)
  assert.equal(parseBinary(huge).reason, "header too large")
  const bad = new Uint8Array(8 + 3)
  new DataView(bad.buffer).setUint32(0, 3)
  bad.set([0x7b, 0x7b, 0x7b], 8)
  assert.equal(parseBinary(bad).reason, "header json")
})

test("audio packets and other headers are told apart from video", () => {
  const audio = new Uint8Array(24)
  new DataView(audio.buffer).setUint32(0, 0x52415532)
  assert.equal(parseBinary(audio).kind, "audio")
  const hdr = new TextEncoder().encode(JSON.stringify({ direction: "app_icon", message: {} }))
  const buf = new Uint8Array(8 + hdr.length)
  new DataView(buf.buffer).setUint32(0, hdr.length)
  buf.set(hdr, 8)
  assert.equal(parseBinary(buf).kind, "other")
})

test("descriptor envelopes: direction/message, type/payload, bare", () => {
  assert.equal(videoDescriptorOf({ direction: "video", message: desc() }).width_px, 640)
  assert.equal(videoDescriptorOf({ type: "video", payload: desc({ keyframe: false }) }).keyframe, false)
  assert.equal(videoDescriptorOf(desc()).codec, "h264")
  assert.equal(videoDescriptorOf({ direction: "server", message: { type: "video_frame", payload: desc({ sequence: 9 }) } }).sequence, 9)
  assert.equal(videoDescriptorOf({ direction: "server", message: { type: "pong", payload: {} } }), null)
  assert.equal(videoDescriptorOf({ direction: "server", message: {} }), null)
  assert.equal(videoDescriptorOf({ direction: "video", message: { codec: 1 } }), null)
})

test("control messages", () => {
  assert.deepEqual(decodeControl('{"type":"hello","payload":{"versions":[2]}}'), { type: "hello", payload: { versions: [2] } })
  assert.deepEqual(decodeControl('{"direction":"server","message":{"type":"pong"}}'), { type: "pong", payload: {} })
  assert.equal(decodeControl('{"direction":"client","message":{"type":"x"}}'), null)
  assert.equal(decodeControl("not json"), null)
  assert.equal(encodeControl("request_keyframe"), '{"type":"request_keyframe"}')
})

test("Annex B: NAL types, keyframes and the codec string", () => {
  assert.deepEqual(nalTypes(au(SPS, PPS, IDR)), [7, 8, 5])
  assert.equal(isAnnexBKeyframe(au(SPS, PPS, IDR)), true)
  assert.equal(isAnnexBKeyframe(au(IDR)), false, "IDR without parameter sets is not decodable alone")
  assert.equal(isAnnexBKeyframe(au(P)), false)
  assert.equal(avcCodecString(au(SPS, PPS, IDR)), "avc1.42c01f")
  assert.equal(avcCodecString(au(P)), null)
  assert.deepEqual(nalTypes(new Uint8Array([1, 2, 3])), [])
})

test("media URL and ticket subprotocols", () => {
  assert.equal(mediaUrl("http://127.0.0.1:3211", "/media?ticket=abc"), "ws://127.0.0.1:3211/media?ticket=abc")
  assert.equal(mediaUrl("https://gw.example/x/", "/media?ticket=t"), "wss://gw.example/media?ticket=t")
  assert.deepEqual(mediaSubprotocols("T"), ["rcdp.v2", "cua.ticket.T"])
})

test("socket state: handshake order, first packet, codec string", () => {
  const s = new MediaSocketState()
  assert.equal(s.handshakeOk, false)
  s.onText('{"type":"hello","payload":{}}')
  s.onText('{"type":"session_opened","payload":{"session_id":"m1"}}')
  s.onText("garbage")
  assert.equal(s.handshakeOk, true)
  assert.equal(s.invalid, 1)
  s.onBinary(encodeVideoPacket(desc(), au(SPS, PPS, IDR)))
  s.onBinary(encodeVideoPacket(desc({ keyframe: false, sequence: 8 }), au(P)))
  s.onBinary(new Uint8Array([1]))
  assert.equal(s.frames, 2)
  assert.equal(s.keyframes, 1)
  assert.equal(s.firstFrameKeyframe, true)
  assert.equal(s.firstFrameAnnexBKeyframe, true)
  assert.equal(s.codecString, "avc1.42c01f")
  assert.equal(s.invalid, 2)
  s.onText('{"type":"error","payload":{"code":"input_sequence_gap"}}')
  assert.equal(s.errors.length, 1)
})

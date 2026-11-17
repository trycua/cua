// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The media plane in the webview: rcdp wire v2 over the spacesd's
// `/media` WebSocket, attached with the ticket the core minted
// (`open_stream`). Video arrives as one length-prefixed binary packet per
// WebSocket message: header_len u32 BE, payload_len u32 BE, the JSON
// WireHeader, then the payload (H.264 Annex B, SPS/PPS on every keyframe).
// Control messages are JSON text messages. See libs/cua/proto/MEDIA.md.

export interface VideoDescriptor {
  session_id: string
  sequence: number
  geometry_epoch: number
  codec_epoch: number
  width_px: number
  height_px: number
  capture_timestamp_us: number
  codec: "h264" | "bgra" | "png"
  keyframe: boolean
}

export interface Packet {
  header: { direction: string; message: unknown }
  payload: Uint8Array
}

/** Parses one binary WebSocket message; throws on a malformed packet. */
export function parsePacket(buf: ArrayBuffer): Packet {
  if (buf.byteLength < 8) throw new Error("short packet")
  const view = new DataView(buf)
  const headerLen = view.getUint32(0, false)
  const payloadLen = view.getUint32(4, false)
  if (headerLen > 1 << 20) throw new Error("header too large")
  if (8 + headerLen + payloadLen !== buf.byteLength) throw new Error("length mismatch")
  const header = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 8, headerLen)))
  return { header, payload: new Uint8Array(buf, 8 + headerLen, payloadLen) }
}

/** The video descriptor, when the packet is video. */
export function videoOf(p: Packet): VideoDescriptor | null {
  return p.header.direction === "video" ? (p.header.message as VideoDescriptor) : null
}

/** NAL units of an Annex B access unit (start codes stripped). */
export function nalUnits(au: Uint8Array): Uint8Array[] {
  const out: Uint8Array[] = []
  let i = 0
  let start = -1
  while (i + 2 < au.length) {
    const three = au[i] === 0 && au[i + 1] === 0 && au[i + 2] === 1
    const four = i + 3 < au.length && au[i] === 0 && au[i + 1] === 0 && au[i + 2] === 0 && au[i + 3] === 1
    if (three || four) {
      if (start >= 0) out.push(au.subarray(start, i))
      i += three ? 3 : 4
      start = i
    } else i++
  }
  if (start >= 0) out.push(au.subarray(start))
  return out
}

/** `avc1.PPCCLL` from the keyframe's SPS, for `VideoDecoder.configure`. */
export function avcCodecString(au: Uint8Array): string | null {
  const sps = nalUnits(au).find((n) => (n[0] & 0x1f) === 7)
  if (!sps || sps.length < 4) return null
  const hex = (b: number) => b.toString(16).padStart(2, "0").toUpperCase()
  return `avc1.${hex(sps[1])}${hex(sps[2])}${hex(sps[3])}`
}

export interface StreamStatus {
  frames: number
  keyframes: number
  firstWasKeyframe: boolean | null
  width: number
  height: number
  state: string
}

/**
 * Attaches to a ticketed media session and paints decoded frames onto
 * `canvas` with WebCodecs. Returns a closer.
 */
export function attach(
  wsUrl: string,
  canvas: HTMLCanvasElement,
  onStatus: (s: StreamStatus) => void,
): () => void {
  const status: StreamStatus = { frames: 0, keyframes: 0, firstWasKeyframe: null, width: 0, height: 0, state: "connecting" }
  const ctx = canvas.getContext("2d")!
  let decoder: VideoDecoder | null = null
  let epoch = -1
  const ws = new WebSocket(wsUrl, ["rcdp.v2"])
  ws.binaryType = "arraybuffer"
  const emit = () => onStatus({ ...status })
  ws.onopen = () => {
    status.state = "attached"
    emit()
  }
  ws.onclose = (e) => {
    status.state = `closed (${e.code})`
    emit()
  }
  ws.onmessage = (ev) => {
    if (typeof ev.data === "string") return // control: hello, session_opened, lifecycle, stats
    let p: Packet
    try {
      p = parsePacket(ev.data as ArrayBuffer)
    } catch {
      return
    }
    const v = videoOf(p)
    if (!v || v.codec !== "h264") return
    status.frames++
    if (status.firstWasKeyframe === null) status.firstWasKeyframe = v.keyframe
    if (v.keyframe) status.keyframes++
    if (v.keyframe && (decoder === null || v.codec_epoch !== epoch)) {
      const codec = avcCodecString(p.payload)
      if (!codec) return
      decoder?.close()
      decoder = new VideoDecoder({
        output: (frame) => {
          if (canvas.width !== frame.displayWidth) canvas.width = frame.displayWidth
          if (canvas.height !== frame.displayHeight) canvas.height = frame.displayHeight
          ctx.drawImage(frame, 0, 0)
          frame.close()
        },
        error: (e) => {
          status.state = `decoder: ${e.message}`
          decoder = null
          emit()
        },
      })
      decoder.configure({ codec, optimizeForLatency: true })
      epoch = v.codec_epoch
    }
    if (!decoder) return // wait for a keyframe
    status.width = v.width_px
    status.height = v.height_px
    decoder.decode(
      new EncodedVideoChunk({
        type: v.keyframe ? "key" : "delta",
        timestamp: v.capture_timestamp_us,
        data: p.payload,
      }),
    )
    emit()
  }
  return () => {
    ws.close(1000)
    decoder?.close()
  }
}

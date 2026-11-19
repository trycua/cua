/**
 * rcdp wire v2, the pure parts (libs/cua/proto/MEDIA.md): what the web UI
 * needs to turn `/media` WebSocket messages into WebCodecs input.
 *
 * Side-effect free and DOM-free, so the web UI (through Vite) and the Node
 * scenario runner (through `dist/core/wire.js`) run the same code, and the
 * unit tests need no socket or decoder.
 *
 * - Binary messages: a length-prefixed video packet (`header_len: u32`,
 *   `payload_len: u32`, big-endian; JSON header; payload) or an RAU2 audio
 *   packet (first byte `R`), which this sample ignores.
 * - Text messages: JSON control `{type, payload}`.
 * - H.264 payloads are one Annex B access unit; keyframes carry SPS/PPS.
 */

/** `VideoFrameDescriptor` (unchanged from v1). */
export interface VideoDescriptor {
  session_id?: string
  sequence: number
  geometry_epoch?: number
  codec_epoch?: number
  width_px: number
  height_px: number
  capture_timestamp_us?: number
  codec: string
  keyframe: boolean
}

export type BinaryPacket =
  | { kind: "video"; descriptor: VideoDescriptor; payload: Uint8Array }
  | { kind: "audio"; bytes: number }
  | { kind: "other"; header: unknown }
  | { kind: "invalid"; reason: string }

export const MAX_HEADER_BYTES = 1 << 20
export const MAX_PAYLOAD_BYTES = 64 << 20
const AUDIO_MAGIC = 0x52415532 // "RAU2"

function asBytes(data: ArrayBuffer | ArrayBufferView): Uint8Array {
  return data instanceof ArrayBuffer
    ? new Uint8Array(data)
    : new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
}

/** Demultiplex one binary WebSocket message. Never throws. */
export function parseBinary(data: ArrayBuffer | ArrayBufferView): BinaryPacket {
  const bytes = asBytes(data)
  if (bytes.byteLength === 0) return { kind: "invalid", reason: "empty" }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  if (bytes.byteLength >= 4 && view.getUint32(0, false) === AUDIO_MAGIC) {
    return { kind: "audio", bytes: bytes.byteLength }
  }
  if (bytes.byteLength < 8) return { kind: "invalid", reason: "short packet" }
  const headerLength = view.getUint32(0, false)
  const payloadLength = view.getUint32(4, false)
  if (headerLength > MAX_HEADER_BYTES) return { kind: "invalid", reason: "header too large" }
  if (payloadLength > MAX_PAYLOAD_BYTES) return { kind: "invalid", reason: "payload too large" }
  if (8 + headerLength + payloadLength !== bytes.byteLength) {
    return { kind: "invalid", reason: "length mismatch" }
  }
  let header: Record<string, unknown>
  try {
    header = JSON.parse(new TextDecoder().decode(bytes.subarray(8, 8 + headerLength)))
  } catch {
    return { kind: "invalid", reason: "header json" }
  }
  const descriptor = videoDescriptorOf(header)
  if (!descriptor) return { kind: "other", header }
  return { kind: "video", descriptor, payload: bytes.subarray(8 + headerLength) }
}

/** The descriptor from a packet header: `{direction:"video", message}` (the
 * driver's envelope) or `{type:"video", payload}`, or a bare descriptor. */
export function videoDescriptorOf(header: Record<string, unknown>): VideoDescriptor | null {
  let h = header
  // `{"direction":"server","message":{"type":"video_frame","payload":…}}`
  // (the form cua-test-fixtures' scripted socket sends) unwraps to the
  // `{type, payload}` form.
  if (h.direction === "server" && h.message && typeof h.message === "object") {
    h = h.message as Record<string, unknown>
  }
  let inner: unknown = h
  if (h.direction !== undefined) {
    if (h.direction !== "video") return null
    inner = h.message
  } else if (h.type !== undefined) {
    if (h.type !== "video" && h.type !== "video_frame") return null
    inner = h.payload
  }
  if (!inner || typeof inner !== "object") return null
  const d = inner as Record<string, unknown>
  if (typeof d.codec !== "string" || typeof d.keyframe !== "boolean") return null
  return {
    session_id: typeof d.session_id === "string" ? d.session_id : undefined,
    sequence: Number(d.sequence ?? 0),
    geometry_epoch: d.geometry_epoch === undefined ? undefined : Number(d.geometry_epoch),
    codec_epoch: d.codec_epoch === undefined ? undefined : Number(d.codec_epoch),
    width_px: Number(d.width_px ?? 0),
    height_px: Number(d.height_px ?? 0),
    capture_timestamp_us: d.capture_timestamp_us === undefined ? undefined : Number(d.capture_timestamp_us),
    codec: d.codec,
    keyframe: d.keyframe,
  }
}

/** Build a length-prefixed video packet (tests). */
export function encodeVideoPacket(descriptor: VideoDescriptor, payload: Uint8Array): Uint8Array {
  const header = new TextEncoder().encode(JSON.stringify({ direction: "video", message: descriptor }))
  const out = new Uint8Array(8 + header.byteLength + payload.byteLength)
  const view = new DataView(out.buffer)
  view.setUint32(0, header.byteLength, false)
  view.setUint32(4, payload.byteLength, false)
  out.set(header, 8)
  out.set(payload, 8 + header.byteLength)
  return out
}

export interface ControlMessage {
  type: string
  payload: Record<string, unknown>
}

/** A server text frame: `{type, payload}`. Null for anything else. */
export function decodeControl(text: string): ControlMessage | null {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    return null
  }
  if (!value || typeof value !== "object") return null
  let o = value as Record<string, unknown>
  if ("direction" in o) {
    if (o.direction !== "server") return null
    o = (o.message ?? {}) as Record<string, unknown>
  }
  if (typeof o.type !== "string") return null
  const payload = o.payload && typeof o.payload === "object" ? (o.payload as Record<string, unknown>) : {}
  return { type: o.type, payload }
}

/** A client text frame. */
export function encodeControl(type: string, payload?: unknown): string {
  return JSON.stringify(payload === undefined ? { type } : { type, payload })
}

// ------------------------------------------------------------ Annex B

export const NAL_IDR = 5
export const NAL_SPS = 7
export const NAL_PPS = 8

/** NAL units of one Annex B access unit (views into `au`). Bounded by the
 * input length: every iteration consumes at least one byte. */
export function nalUnits(au: Uint8Array): Uint8Array[] {
  const starts: Array<{ at: number; len: number }> = []
  let i = 0
  while (i + 2 < au.length) {
    if (au[i] === 0 && au[i + 1] === 0 && au[i + 2] === 1) {
      starts.push({ at: i + 3, len: 3 })
      i += 3
    } else if (i + 3 < au.length && au[i] === 0 && au[i + 1] === 0 && au[i + 2] === 0 && au[i + 3] === 1) {
      starts.push({ at: i + 4, len: 4 })
      i += 4
    } else {
      i += 1
    }
  }
  return starts.map((s, k) => {
    const end = k + 1 < starts.length ? starts[k + 1].at - starts[k + 1].len : au.length
    return au.subarray(s.at, end)
  })
}

export function nalTypes(au: Uint8Array): number[] {
  return nalUnits(au)
    .filter((n) => n.length > 0)
    .map((n) => n[0] & 0x1f)
}

/** A decodable random-access point: an IDR slice with SPS and PPS before it. */
export function isAnnexBKeyframe(au: Uint8Array): boolean {
  const types = nalTypes(au)
  const idr = types.indexOf(NAL_IDR)
  return idr >= 0 && types.slice(0, idr).includes(NAL_SPS) && types.slice(0, idr).includes(NAL_PPS)
}

/** WebCodecs codec string (`avc1.PPCCLL`) from the access unit's SPS. */
export function avcCodecString(au: Uint8Array): string | null {
  const sps = nalUnits(au).find((n) => n.length >= 4 && (n[0] & 0x1f) === NAL_SPS)
  if (!sps) return null
  const hex = (b: number) => b.toString(16).padStart(2, "0")
  return `avc1.${hex(sps[1])}${hex(sps[2])}${hex(sps[3])}`
}

// ------------------------------------------------------------ session

export const MEDIA_SUBPROTOCOL = "rcdp.v2"

/** Browser-safe subprotocols carrying the ticket (MEDIA.md §2). */
export function mediaSubprotocols(ticket: string): string[] {
  return [MEDIA_SUBPROTOCOL, `cua.ticket.${ticket}`]
}

/** The `/media` URL for a driver base URL and `OpenMedia`'s `ws_path`. */
export function mediaUrl(baseUrl: string, wsPath: string): string {
  const u = new URL(wsPath, baseUrl)
  u.protocol = u.protocol === "https:" ? "wss:" : u.protocol === "http:" ? "ws:" : u.protocol
  return u.toString()
}

/**
 * The state machine a client runs over one socket: server-first handshake
 * (`hello` then `session_opened`), then video. Feeds `on*` from socket
 * events; `take()` hands out what happened since the last call.
 */
export class MediaSocketState {
  hello: ControlMessage | null = null
  opened: ControlMessage | null = null
  frames = 0
  keyframes = 0
  firstFrameKeyframe: boolean | null = null
  firstFrameAnnexBKeyframe: boolean | null = null
  codecString: string | null = null
  invalid = 0
  errors: string[] = []
  closed: { code: number; reason: string } | null = null

  onText(text: string): void {
    const m = decodeControl(text)
    if (!m) {
      this.invalid += 1
      return
    }
    if (m.type === "hello" && !this.hello) this.hello = m
    else if (m.type === "session_opened" && !this.opened) this.opened = m
    else if (m.type === "error") this.errors.push(JSON.stringify(m.payload))
  }

  onBinary(data: ArrayBuffer | ArrayBufferView): VideoDescriptor | null {
    const p = parseBinary(data)
    if (p.kind === "invalid") {
      this.invalid += 1
      return null
    }
    if (p.kind !== "video") return null
    if (this.firstFrameKeyframe === null) {
      this.firstFrameKeyframe = p.descriptor.keyframe
      this.firstFrameAnnexBKeyframe = p.descriptor.codec === "h264" ? isAnnexBKeyframe(p.payload) : null
    }
    if (p.descriptor.codec === "h264" && p.descriptor.keyframe && !this.codecString) {
      this.codecString = avcCodecString(p.payload)
    }
    this.frames += 1
    if (p.descriptor.keyframe) this.keyframes += 1
    return p.descriptor
  }

  onClose(code: number, reason: string): void {
    this.closed = { code, reason }
  }

  /** The handshake order MEDIA.md §3 requires. */
  get handshakeOk(): boolean {
    return this.hello !== null && this.opened !== null
  }
}

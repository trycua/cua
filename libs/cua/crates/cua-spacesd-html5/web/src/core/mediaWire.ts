// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * rcdp wire v2 — the pure parts (libs/cua/proto/MEDIA.md).
 *
 * Everything here is side-effect free so the protocol rules are unit-testable
 * without a socket, a decoder or an AudioContext:
 *
 * - binary demux: RAU2 audio packets (first byte 0x52) vs length-prefixed
 *   video packets (first byte 0x00, MEDIA.md §12.2);
 * - control text decode (bare `{type, payload}`, plus the v1
 *   `{direction, message}` envelope still accepted during migration);
 * - u32 serial-number arithmetic for audio sequences (RFC 1982, §12.4);
 * - the interactive-input sequencer (adopt any base, contiguous batches,
 *   resync on `input_sequence_gap`, §6);
 * - the close-code policy (§10) and ticket expiry;
 * - the adaptive audio jitter-buffer target (20–60 ms, §12.5);
 * - DOM input → `InteractiveInputEvent` mapping.
 */

// ---------------------------------------------------------------- framing

export const AUDIO_MAGIC = 0x52415532; // "RAU2"
export const AUDIO_HEADER_BYTES = 24;

/** `VideoFrameDescriptor` (unchanged from v1). */
export interface VideoDescriptor {
  session_id: string;
  sequence: number;
  geometry_epoch: number;
  codec_epoch: number;
  width_px: number;
  height_px: number;
  capture_timestamp_us: number;
  codec: string;
  keyframe: boolean;
}

export interface AudioPacketHeader {
  flags: number;
  discontinuity: boolean;
  dtx: boolean;
  trackId: number;
  sequence: number;
  ptsUs: number;
  frameSamples: number;
  configEpoch: number;
}

export type BinaryPacket =
  | { kind: "audio"; header: AudioPacketHeader; payload: Uint8Array }
  | { kind: "video"; descriptor: VideoDescriptor; payload: Uint8Array }
  | { kind: "other"; direction: string }
  | { kind: "invalid"; reason: string };

/** Parse the fixed 24-byte RAU2 header; null when the packet must be dropped
 * (bad magic, version, reserved bits, or too short). */
export function parseAudioHeader(buffer: ArrayBuffer): AudioPacketHeader | null {
  if (buffer.byteLength < AUDIO_HEADER_BYTES) return null;
  const view = new DataView(buffer);
  if (view.getUint32(0, false) !== AUDIO_MAGIC) return null;
  if (view.getUint8(4) !== 2) return null;
  const flags = view.getUint8(5);
  if ((flags & 0xfc) !== 0) return null; // bits 2–7 reserved
  if (view.getUint8(23) !== 0) return null;
  const trackId = view.getUint16(6, false);
  if (trackId === 0) return null;
  return {
    flags,
    discontinuity: (flags & 0x01) !== 0,
    dtx: (flags & 0x02) !== 0,
    trackId,
    sequence: view.getUint32(8, false),
    ptsUs: Number(view.getBigUint64(12, false)),
    frameSamples: view.getUint16(20, false),
    configEpoch: view.getUint8(22),
  };
}

/** Build an RAU2 packet (tests and the uplink path). */
export function encodeAudioPacket(header: Omit<AudioPacketHeader, "discontinuity" | "dtx" | "flags"> & {
  flags?: number;
}, payload: Uint8Array): ArrayBuffer {
  const buffer = new ArrayBuffer(AUDIO_HEADER_BYTES + payload.byteLength);
  const view = new DataView(buffer);
  view.setUint32(0, AUDIO_MAGIC, false);
  view.setUint8(4, 2);
  view.setUint8(5, header.flags ?? 0);
  view.setUint16(6, header.trackId, false);
  view.setUint32(8, header.sequence >>> 0, false);
  view.setBigUint64(12, BigInt(Math.max(0, Math.floor(header.ptsUs))), false);
  view.setUint16(20, header.frameSamples, false);
  view.setUint8(22, header.configEpoch & 0xff);
  view.setUint8(23, 0);
  new Uint8Array(buffer, AUDIO_HEADER_BYTES).set(payload);
  return buffer;
}

/** Build a length-prefixed video packet (tests). */
export function encodeVideoPacket(descriptor: VideoDescriptor, payload: Uint8Array): ArrayBuffer {
  const header = new TextEncoder().encode(JSON.stringify({ direction: "video", message: descriptor }));
  const buffer = new ArrayBuffer(8 + header.byteLength + payload.byteLength);
  const view = new DataView(buffer);
  view.setUint32(0, header.byteLength, false);
  view.setUint32(4, payload.byteLength, false);
  const bytes = new Uint8Array(buffer);
  bytes.set(header, 8);
  bytes.set(payload, 8 + header.byteLength);
  return buffer;
}

/** Demultiplex one binary WebSocket message. */
export function parseBinary(buffer: ArrayBuffer): BinaryPacket {
  if (buffer.byteLength < 1) return { kind: "invalid", reason: "empty" };
  const first = new Uint8Array(buffer, 0, 1)[0];
  if (first === 0x52) {
    const header = parseAudioHeader(buffer);
    if (!header) return { kind: "invalid", reason: "bad audio header" };
    return { kind: "audio", header, payload: new Uint8Array(buffer, AUDIO_HEADER_BYTES) };
  }
  if (buffer.byteLength < 8) return { kind: "invalid", reason: "short packet" };
  const view = new DataView(buffer);
  const headerLength = view.getUint32(0, false);
  const payloadLength = view.getUint32(4, false);
  if (headerLength > 1 << 20) return { kind: "invalid", reason: "header too large" };
  if (8 + headerLength + payloadLength !== buffer.byteLength) {
    return { kind: "invalid", reason: "length mismatch" };
  }
  let header: { direction?: string; message?: unknown };
  try {
    header = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, 8, headerLength)));
  } catch {
    return { kind: "invalid", reason: "header json" };
  }
  if (header.direction !== "video") return { kind: "other", direction: String(header.direction) };
  return {
    kind: "video",
    descriptor: header.message as VideoDescriptor,
    payload: new Uint8Array(buffer, 8 + headerLength, payloadLength),
  };
}

export interface ControlMessage {
  type: string;
  payload: Record<string, unknown>;
}

/** Decode a server text frame: bare `{type, payload}` (v2) or the v1
 * `{direction: "server", message}` envelope. Null for anything else. */
export function decodeControl(text: string): ControlMessage | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object") return null;
  let object = value as Record<string, unknown>;
  if ("direction" in object) {
    if (object.direction !== "server") return null;
    object = (object.message ?? {}) as Record<string, unknown>;
  }
  if (typeof object.type !== "string") return null;
  const payload = object.payload && typeof object.payload === "object" ? object.payload : {};
  return { type: object.type, payload: payload as Record<string, unknown> };
}

/** Encode a client text frame (bare v2 shape). */
export function encodeControl(type: string, payload?: unknown): string {
  return JSON.stringify(payload === undefined ? { type } : { type, payload });
}

// ---------------------------------------------------------------- sequences

/** `b - a` as a signed 32-bit serial distance (RFC 1982). Positive when `b`
 * is after `a`. */
export function serialDiff32(a: number, b: number): number {
  return ((b - a) | 0);
}

export type AudioSequenceVerdict =
  | { kind: "first" }
  | { kind: "next" }
  | { kind: "gap"; lost: number }
  | { kind: "late" };

/** Classify an audio packet's sequence against the last one played. A gap is
 * loss (DTX silence does not advance the sequence, §12.4); a packet at or
 * before the last is late/duplicate and must be dropped, never played. */
export function classifyAudioSequence(last: number | null, next: number): AudioSequenceVerdict {
  if (last === null) return { kind: "first" };
  const d = serialDiff32(last, next);
  if (d <= 0) return { kind: "late" };
  if (d === 1) return { kind: "next" };
  return { kind: "gap", lost: d - 1 };
}

/**
 * Interactive-input sequencer (§6): the first batch on a socket may start
 * anywhere and the server adopts it; later batches must be contiguous. On
 * `input_sequence_gap` the error carries `expected_sequence` and the client
 * re-syncs to it instead of failing every following batch.
 */
export class InputSequencer {
  private next: number;

  constructor(start = Math.floor(Math.random() * 1e9) + 1) {
    this.next = start;
  }

  /** Reserve `count` sequence numbers; returns the batch's first sequence. */
  take(count: number): number {
    const first = this.next;
    this.next += Math.max(0, count);
    return first;
  }

  /** Server said `input_sequence_gap`. */
  resync(expected: number | undefined): void {
    if (typeof expected === "number" && Number.isFinite(expected) && expected > 0) {
      this.next = expected;
    }
  }

  peek(): number {
    return this.next;
  }
}

/** Largest interactive-input batch (protocol bound). */
export const MAX_INPUT_BATCH = 256;

// ---------------------------------------------------------------- lifecycle

export type CloseAction =
  /** Attach again with the same ticket (network blip) while it is valid. */
  | "reattach"
  /** Mint a new ticket through `open_space_stream` and attach. */
  | "reopen"
  /** The session or target is gone for good. */
  | "gone"
  /** We closed it. */
  | "stop";

/** What to do after the socket closed with `code` (MEDIA.md §10). */
export function closeAction(code: number, ticketValid: boolean): CloseAction {
  switch (code) {
    case 1000:
      return "stop";
    case 4401: // ticket invalid / token rotated
      return "reopen";
    case 4404: // session closed (CloseMedia) or expired
    case 4410: // target gone
      return "gone";
    case 4429: // too many sockets on the session
      return "gone";
    case 4400:
    case 4500:
      return "reopen";
    default:
      // 1006 / 4408 / network: the ticket may reattach until it expires.
      return ticketValid ? "reattach" : "reopen";
  }
}

/** Whether a ticket (RFC 3339 expiry, or none) can still attach at `nowMs`,
 * with a safety margin for the round trip. */
export function ticketValid(expiresAt: string | undefined | null, nowMs: number, marginMs = 2_000): boolean {
  if (!expiresAt) return true;
  const at = Date.parse(expiresAt);
  if (!Number.isFinite(at)) return true;
  return at - marginMs > nowMs;
}

// ---------------------------------------------------------------- jitter

export const JITTER_MIN_MS = 20;
export const JITTER_MAX_MS = 60;
/** Ceiling for TCP paths (gateway, relay), where head-of-line blocking can
 * exceed 60 ms; the target is reported in stats when above JITTER_MAX_MS. */
export const JITTER_TCP_CEILING_MS = 200;

/**
 * Adaptive jitter-buffer target for one downlink track (§12.5): starts at
 * 2 × frame_ms clamped to 20–60, follows ~2 × the RFC 3550 interarrival jitter
 * estimate + one frame, grows immediately on underrun, and shrinks slowly
 * (≤ 5 ms/s) and only while the track is silent.
 */
export class JitterController {
  private jitterMs = 0;
  private targetMs: number;
  private lastTransit: number | null = null;
  private lastShrinkAt: number | null = null;

  constructor(
    private readonly frameMs: number,
    private readonly ceilingMs = JITTER_TCP_CEILING_MS,
  ) {
    this.targetMs = clamp(2 * frameMs, JITTER_MIN_MS, JITTER_MAX_MS);
  }

  get target(): number {
    return this.targetMs;
  }

  get jitter(): number {
    return this.jitterMs;
  }

  /** One packet arrived at `arrivalMs` (local clock) carrying `ptsUs`. */
  onPacket(ptsUs: number, arrivalMs: number): void {
    const transit = arrivalMs - ptsUs / 1000;
    if (this.lastTransit !== null) {
      const d = Math.abs(transit - this.lastTransit);
      this.jitterMs += (d - this.jitterMs) / 16;
    }
    this.lastTransit = transit;
    const wanted = 2 * this.jitterMs + this.frameMs;
    if (wanted > this.targetMs) this.targetMs = Math.min(this.ceilingMs, wanted);
  }

  /** Playout ran dry: grow at once. */
  onUnderrun(): void {
    this.targetMs = Math.min(this.ceilingMs, this.targetMs + this.frameMs);
  }

  /** Called periodically while the track is silent/DTX; shrinks by at most
   * 5 ms per second of elapsed time toward the jitter-derived floor. */
  onSilence(nowMs: number): void {
    const last = this.lastShrinkAt ?? nowMs;
    this.lastShrinkAt = nowMs;
    const elapsedS = Math.max(0, (nowMs - last) / 1000);
    const floor = clamp(Math.max(2 * this.jitterMs + this.frameMs, JITTER_MIN_MS), JITTER_MIN_MS, this.ceilingMs);
    if (this.targetMs > floor) {
      this.targetMs = Math.max(floor, this.targetMs - 5 * elapsedS);
    }
  }
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

// ---------------------------------------------------------------- input

export type InputModifier = "command" | "shift" | "option" | "control";

export type InteractiveInputEvent =
  | { kind: "text_commit"; text: string }
  | { kind: "key"; key: string; state: "down" | "up"; modifiers: InputModifier[]; repeat: boolean }
  | {
      kind: "pointer";
      phase: "move" | "down" | "up" | "cancel";
      button: "left" | "right" | "middle" | null;
      x_normalized: number;
      y_normalized: number;
      modifiers: InputModifier[];
    }
  | {
      kind: "scroll";
      x_normalized: number;
      y_normalized: number;
      delta_x: number;
      delta_y: number;
      phase: "none";
      momentum_phase: "none";
      precise: boolean;
    };

interface ModifierState {
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

export function modifiersOf(event: ModifierState): InputModifier[] {
  const out: InputModifier[] = [];
  if (event.shiftKey) out.push("shift");
  if (event.ctrlKey) out.push("control");
  if (event.altKey) out.push("option");
  if (event.metaKey) out.push("command");
  return out;
}

/** Normalized [0,1] position of a client point over a rect. */
export function normalizePoint(
  clientX: number,
  clientY: number,
  rect: { left: number; top: number; width: number; height: number },
): { x_normalized: number; y_normalized: number } {
  const w = rect.width || 1;
  const h = rect.height || 1;
  return {
    x_normalized: clamp((clientX - rect.left) / w, 0, 1),
    y_normalized: clamp((clientY - rect.top) / h, 0, 1),
  };
}

const BUTTONS = ["left", "middle", "right"] as const;

export function pointerButton(button: number): "left" | "right" | "middle" {
  return BUTTONS[button] ?? "left";
}

/** A keydown/keyup → input event. Plain printable characters are committed as
 * text on keydown (and produce nothing on keyup); everything else is a key. */
export function keyEventToInput(
  event: ModifierState & { key: string; repeat: boolean },
  state: "down" | "up",
): InteractiveInputEvent | null {
  const printable = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
  if (printable) return state === "down" ? { kind: "text_commit", text: event.key } : null;
  return {
    kind: "key",
    key: event.key === " " ? "space" : event.key.toLowerCase(),
    state,
    modifiers: modifiersOf(event),
    repeat: state === "down" ? event.repeat : false,
  };
}

/** Coalesce adjacent text commits without crossing other events (keeps a
 * burst of typing to one event). */
export function coalesceText(events: readonly InteractiveInputEvent[]): InteractiveInputEvent[] {
  const out: InteractiveInputEvent[] = [];
  for (const event of events) {
    const prev = out[out.length - 1];
    if (event.kind === "text_commit" && prev?.kind === "text_commit") {
      out[out.length - 1] = { kind: "text_commit", text: prev.text + event.text };
    } else {
      out.push(event);
    }
  }
  return out;
}

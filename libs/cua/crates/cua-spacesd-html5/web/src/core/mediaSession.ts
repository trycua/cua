// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * One rcdp wire v2 media session rendered into a canvas (MEDIA.md).
 *
 * Session setup is gRPC (`open_space_stream` in the shell mints a ticket);
 * this class only attaches the ticketed WebSocket and plays what arrives:
 *
 * - server-first handshake: `hello`, `session_opened`, then one
 *   `audio_config` per audio track — the client never speaks first;
 * - keyframe on attach: the first video packet on every socket is a current
 *   keyframe, so decoding starts without asking; `request_keyframe` is only
 *   sent after a decoder loss (throttled to 1/s);
 * - frame sequences may start anywhere; `codec_epoch` changes rebuild the
 *   H.264 decoder from the next keyframe; BGRA/PNG need no WebCodecs;
 * - interactive input with the v2 sequencing rules (adopt any base, resync
 *   on `input_sequence_gap`);
 * - disconnects reattach with the SAME ticket until it expires, then a fresh
 *   ticket is minted; 4401 re-opens, 4404/4410 end the session.
 */

import { AudioPlayer, type AudioConfigMessage } from "./audio";

export type { AudioConfigMessage };
import { avcCodecFromAnnexB } from "./h264";
import {
  closeAction,
  coalesceText,
  decodeControl,
  encodeControl,
  InputSequencer,
  keyEventToInput,
  MAX_INPUT_BATCH,
  modifiersOf,
  normalizePoint,
  parseBinary,
  pointerButton,
  ticketValid,
  type InteractiveInputEvent,
  type VideoDescriptor,
} from "./mediaWire";

/** Where to attach: a ticketed `ws(s)://…/media?ticket=…` URL. */
export interface MediaTicket {
  wsUrl: string;
  ticketExpiresAt?: string;
  mediaSessionId?: string;
}

export type MediaStatus = "connecting" | "streaming" | "reconnecting" | "ended" | "failed";

export interface SessionOpenedPayload {
  session_id: string;
  target?: { kind?: string; handle?: string; display_id?: string };
  geometry?: { width_px: number; height_px: number; scale_factor?: number };
  geometry_epoch?: number;
  codec?: string;
  policy?: string;
  geometry_control?: string;
  capabilities?: string[];
}

export interface MediaSessionOptions {
  canvas: HTMLCanvasElement;
  /** The first ticket. */
  ticket: MediaTicket;
  /** Mint a fresh ticket (expiry, 4401). Absent for a borrowed ticket
   * (`targetWindow.mediaUrl`): the session then ends instead of re-opening. */
  reopen?: () => Promise<MediaTicket>;
  /** Forward pointer/keyboard input (never for PiP / view-only). */
  interactive: boolean;
  /** Listen for pointer/keyboard events on the canvas itself (default
   * true). The HTML5 viewer has its own input controller and passes false,
   * then feeds events through `queueInput`. */
  wireInput?: boolean;
  /** Play downlink audio tracks the session negotiated. */
  audio: boolean;
  onStatus?: (status: MediaStatus, detail?: string) => void;
  onOpened?: (opened: SessionOpenedPayload) => void;
  /** Authoritative frame geometry (session_opened / geometry_changed). */
  onGeometry?: (width: number, height: number) => void;
  onTitle?: (title: string) => void;
  onSuspended?: (reason: string | null) => void;
  /** `window_geometry_result` (single-window resize sync). */
  onGeometryResult?: (result: { applied: boolean; width_points: number; height_points: number }) => void;
  /** Ended for good (4404/4410, lifecycle closed, or a borrowed ticket expired). */
  onGone?: (reason: string) => void;
  /** Every `audio_config` (downlink and uplink tracks). */
  onAudioConfig?: (config: AudioConfigMessage) => void;
  /** A server `error` control message. */
  onServerError?: (payload: Record<string, unknown>) => void;
  /** A frame was drawn (after decode). */
  onFrame?: (width: number, height: number) => void;
}

/** Counters for the stats overlay and tests. */
export interface MediaCounters {
  framesReceived: number;
  framesDecoded: number;
  keyframes: number;
  bytesReceived: number;
  audioPackets: number;
  /** `performance.now()` of the last drawn frame, or 0. */
  lastFrameAt: number;
  /** Mean decode time (ms) of the last frames, WebCodecs only. */
  decodeMs: number;
  codec: string;
}

const RECONNECT_MS = 1500;
/** Consecutive failed attaches before giving up (then a user retry restarts). */
const MAX_ATTACH_FAILURES = 20;
/** Minimum gap between `request_keyframe` messages. */
const KEYFRAME_RETRY_MS = 1000;
/** A decoder with more than this many queued frames is overloaded. */
const MAX_DECODE_QUEUE = 4;
const POINTER_MOVE_INTERVAL_MS = 16;

export class MediaSession {
  private socket: WebSocket | null = null;
  private stopped = false;
  private ticket: MediaTicket;
  private reconnectTimer: number | null = null;
  private failures = 0;
  private sessionId = "";
  private opened: SessionOpenedPayload | null = null;
  private helloCapabilities: string[] = [];

  // video
  private readonly context: CanvasRenderingContext2D | null;
  private decoder: VideoDecoder | null = null;
  private decoderEpoch = -1;
  private awaitingKeyframe = true;
  private lastKeyframeRequest = Number.NEGATIVE_INFINITY;
  private lastSequence: number | null = null;
  private readonly sequenceByTimestamp = new Map<number, number>();
  private imageData: ImageData | null = null;
  framesDecoded = 0;
  private readonly counters: MediaCounters = {
    framesReceived: 0,
    framesDecoded: 0,
    keyframes: 0,
    bytesReceived: 0,
    audioPackets: 0,
    lastFrameAt: 0,
    decodeMs: 0,
    codec: "",
  };
  private readonly decodeStartedAt = new Map<number, number>();

  // input
  private readonly sequencer = new InputSequencer();
  private pending: InteractiveInputEvent[] = [];
  private flushScheduled = false;
  private lastMove = 0;
  private readonly detachInput: Array<() => void> = [];
  private geometryRevision = 0;

  // audio
  private readonly audioPlayer: AudioPlayer | null;

  constructor(private readonly options: MediaSessionOptions) {
    this.ticket = options.ticket;
    this.context = safeContext(options.canvas);
    this.audioPlayer = options.audio ? new AudioPlayer() : null;
    if (options.interactive && options.wireInput !== false) this.wireInput();
  }

  /** Counters (a copy). */
  get stats(): MediaCounters {
    return { ...this.counters, framesDecoded: this.framesDecoded };
  }

  /** Downlink audio track stats. */
  audioStats() {
    return this.audioPlayer?.stats() ?? [];
  }

  /** Whether the socket is attached and the session opened. */
  get attached(): boolean {
    return this.socket?.readyState === WebSocket.OPEN && this.sessionId !== "";
  }

  /** Send a binary media message (uplink RAU2 audio packets). */
  sendBinary(data: ArrayBuffer): boolean {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(data);
    return true;
  }

  /** Send a control message on the media socket. */
  sendControl(type: string, payload?: unknown): void {
    this.send(type, payload);
  }

  /** Ask for a fresh keyframe (throttled). */
  keyframe(): void {
    this.requestKeyframe();
  }

  /** Current media session id (from `session_opened`). */
  get id(): string {
    return this.sessionId || this.ticket.mediaSessionId || "";
  }

  get sessionOpened(): SessionOpenedPayload | null {
    return this.opened;
  }

  /** Sequence of the last video frame received (they start anywhere). */
  get lastFrameSequence(): number | null {
    return this.lastSequence;
  }

  get serverCapabilities(): readonly string[] {
    return this.helloCapabilities;
  }

  start(): void {
    this.stopped = false;
    this.failures = 0;
    this.options.onStatus?.("connecting");
    this.attach();
  }

  stop(): void {
    this.stopped = true;
    if (this.reconnectTimer !== null) {
      window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    for (const off of this.detachInput.splice(0)) off();
    this.discardDecoder();
    this.audioPlayer?.close();
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.onclose = null;
      try {
        socket.close(1000);
      } catch {
        // already gone
      }
    }
  }

  setAudioMuted(muted: boolean): void {
    this.audioPlayer?.setMuted(muted);
  }

  /** Ask the host to resize the window (single-window mode with a
   * bidirectional geometry grant). Revisions strictly increase. */
  setWindowGeometry(widthPoints: number, heightPoints: number): boolean {
    if (!this.sessionId || this.opened?.geometry_control !== "bidirectional") return false;
    this.geometryRevision += 1;
    this.send("set_window_geometry", {
      session_id: this.sessionId,
      revision: this.geometryRevision,
      width_points: Math.round(widthPoints),
      height_points: Math.round(heightPoints),
    });
    return true;
  }

  /** Queue an input event (also used by tests and programmatic typing). */
  queueInput(event: InteractiveInputEvent): void {
    if (!this.options.interactive) return;
    this.pending.push(event);
    if (this.flushScheduled) return;
    this.flushScheduled = true;
    queueMicrotask(() => {
      this.flushScheduled = false;
      this.flushInput();
    });
  }

  // ---------------------------------------------------------------- socket

  private attach(): void {
    if (this.stopped) return;
    let socket: WebSocket;
    try {
      // The ticket in the URL authenticates the upgrade (MEDIA.md §2);
      // offering `rcdp.v2` lets the server confirm the wire version.
      socket = new WebSocket(this.ticket.wsUrl, ["rcdp.v2"]);
    } catch {
      this.scheduleRetry(false);
      return;
    }
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    socket.onmessage = (event: MessageEvent) => {
      try {
        if (typeof event.data === "string") this.handleControl(event.data);
        else this.handleBinary(event.data as ArrayBuffer);
      } catch {
        // A malformed frame must not tear the session down.
      }
    };
    socket.onclose = (event: CloseEvent) => this.onClose(event?.code ?? 1006);
    socket.onerror = () => {
      // `onclose` follows and decides.
    };
  }

  private onClose(code: number): void {
    this.socket = null;
    if (this.stopped) return;
    // A fresh attach resends a keyframe; drop decoder state tied to this socket.
    this.discardDecoder();
    this.awaitingKeyframe = true;
    const valid = ticketValid(this.ticket.ticketExpiresAt, Date.now());
    const action = closeAction(code, valid);
    if (action === "stop") {
      this.options.onStatus?.("ended");
      return;
    }
    if (action === "gone") {
      this.stopped = true;
      this.options.onStatus?.("ended", closeReason(code));
      this.options.onGone?.(closeReason(code));
      return;
    }
    this.scheduleRetry(action === "reopen");
  }

  private scheduleRetry(reopen: boolean): void {
    if (this.stopped || this.reconnectTimer !== null) return;
    this.failures += 1;
    if (this.failures > MAX_ATTACH_FAILURES) {
      this.options.onStatus?.("failed", "the stream could not be re-attached");
      return;
    }
    this.options.onStatus?.("reconnecting");
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = null;
      if (this.stopped) return;
      if (!reopen) {
        this.attach();
        return;
      }
      if (!this.options.reopen) {
        this.stopped = true;
        this.options.onStatus?.("ended", "the stream ticket expired");
        this.options.onGone?.("ticket expired");
        return;
      }
      this.options
        .reopen()
        .then((ticket) => {
          if (this.stopped) return;
          this.ticket = ticket;
          this.attach();
        })
        .catch(() => this.scheduleRetry(true));
    }, RECONNECT_MS);
  }

  private send(type: string, payload?: unknown): void {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) return;
    socket.send(encodeControl(type, payload));
  }

  // ---------------------------------------------------------------- control

  private handleControl(text: string): void {
    const message = decodeControl(text);
    if (!message) return;
    const p = message.payload;
    switch (message.type) {
      case "hello":
        this.helloCapabilities = Array.isArray(p.capabilities) ? (p.capabilities as string[]) : [];
        break;
      case "session_opened": {
        const opened = p as unknown as SessionOpenedPayload;
        this.opened = opened;
        this.sessionId = String(opened.session_id ?? "");
        this.failures = 0;
        this.lastSequence = null;
        this.awaitingKeyframe = true;
        this.options.onStatus?.("streaming");
        this.options.onOpened?.(opened);
        if (opened.geometry) this.applyGeometry(opened.geometry.width_px, opened.geometry.height_px);
        break;
      }
      case "audio_config":
        this.audioPlayer?.configure(p as unknown as AudioConfigMessage);
        this.options.onAudioConfig?.(p as unknown as AudioConfigMessage);
        break;
      case "audio_track_state":
        this.audioPlayer?.trackState(Number(p.track_id), String(p.state ?? ""));
        break;
      case "lifecycle":
        this.handleLifecycle((p.event ?? {}) as Record<string, unknown>);
        break;
      case "window_geometry_result":
        this.options.onGeometryResult?.({
          applied: Boolean(p.applied),
          width_points: Number(p.width_points ?? 0),
          height_points: Number(p.height_points ?? 0),
        });
        break;
      case "error":
        if (p.code === "input_sequence_gap") {
          this.sequencer.resync(Number(p.expected_sequence));
        }
        this.options.onServerError?.(p);
        break;
      default:
        break;
    }
  }

  private handleLifecycle(event: Record<string, unknown>): void {
    switch (event.kind) {
      case "geometry_changed": {
        const g = event.geometry as { width_px?: number; height_px?: number } | undefined;
        if (g?.width_px && g.height_px) this.applyGeometry(g.width_px, g.height_px);
        break;
      }
      case "title_changed":
        this.options.onTitle?.(String(event.title ?? ""));
        break;
      case "suspended":
        this.options.onSuspended?.(String(event.reason ?? "suspended"));
        break;
      case "resumed":
        this.options.onSuspended?.(null);
        break;
      case "closed":
        // The target is gone; the server closes with 4410 next.
        this.stopped = true;
        this.options.onStatus?.("ended", "target closed");
        this.options.onGone?.("target closed");
        break;
      default:
        break;
    }
  }

  private applyGeometry(width: number, height: number): void {
    const canvas = this.options.canvas;
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
      this.imageData = null;
    }
    this.options.onGeometry?.(width, height);
  }

  // ---------------------------------------------------------------- media

  private handleBinary(buffer: ArrayBuffer): void {
    const packet = parseBinary(buffer);
    this.counters.bytesReceived += buffer.byteLength;
    if (packet.kind === "audio") {
      this.counters.audioPackets += 1;
      this.audioPlayer?.packet(packet.header, packet.payload);
      return;
    }
    if (packet.kind !== "video") return;
    const d = packet.descriptor;
    if (this.sessionId && String(d.session_id) !== this.sessionId) return;
    this.counters.framesReceived += 1;
    if (d.keyframe) this.counters.keyframes += 1;
    this.counters.codec = d.codec;
    if (d.codec === "bgra") this.drawBgra(d, packet.payload);
    else if (d.codec === "png") this.drawPng(d, packet.payload);
    else if (d.codec === "h264") this.drawH264(d, packet.payload);
    this.lastSequence = d.sequence;
  }

  private drawBgra(d: VideoDescriptor, payload: Uint8Array): void {
    const ctx = this.context;
    if (!ctx) return;
    if (payload.byteLength !== d.width_px * d.height_px * 4) return;
    const canvas = this.options.canvas;
    if (canvas.width !== d.width_px || canvas.height !== d.height_px) {
      canvas.width = d.width_px;
      canvas.height = d.height_px;
      this.imageData = null;
    }
    if (!this.imageData) this.imageData = ctx.createImageData(d.width_px, d.height_px);
    const rgba = this.imageData.data;
    for (let i = 0; i < payload.length; i += 4) {
      rgba[i] = payload[i + 2] ?? 0;
      rgba[i + 1] = payload[i + 1] ?? 0;
      rgba[i + 2] = payload[i] ?? 0;
      rgba[i + 3] = 255;
    }
    ctx.putImageData(this.imageData, 0, 0);
    this.framesDecoded += 1;
    this.frameDrawn(d.width_px, d.height_px);
  }

  private frameDrawn(width: number, height: number): void {
    this.counters.lastFrameAt = performance.now();
    this.options.onFrame?.(width, height);
  }

  private drawPng(d: VideoDescriptor, payload: Uint8Array): void {
    const ctx = this.context;
    if (!ctx || typeof createImageBitmap === "undefined") return;
    const blob = new Blob([payload.slice()], { type: "image/png" });
    void createImageBitmap(blob).then((bitmap) => {
      const canvas = this.options.canvas;
      canvas.width = d.width_px;
      canvas.height = d.height_px;
      ctx.drawImage(bitmap, 0, 0);
      bitmap.close();
      this.framesDecoded += 1;
      this.frameDrawn(d.width_px, d.height_px);
    });
  }

  private drawH264(d: VideoDescriptor, payload: Uint8Array): void {
    if (typeof VideoDecoder === "undefined") return; // no WebCodecs: ask for BGRA/PNG
    const epoch = Number(d.codec_epoch ?? 0);
    if (!this.decoder || this.decoderEpoch !== epoch) {
      if (!d.keyframe) {
        // Keyframe-on-attach means this only happens after a decoder loss or
        // an epoch change without an IDR in sight.
        this.requestKeyframe();
        return;
      }
      if (!this.configureDecoder(epoch, payload)) return;
    }
    const decoder = this.decoder;
    if (!decoder) return;
    if (this.awaitingKeyframe && !d.keyframe) {
      this.requestKeyframe();
      return;
    }
    if (decoder.decodeQueueSize > MAX_DECODE_QUEUE && !d.keyframe) {
      this.awaitingKeyframe = true;
      this.requestKeyframe();
      return;
    }
    if (d.keyframe) this.awaitingKeyframe = false;
    this.sequenceByTimestamp.set(d.capture_timestamp_us, d.sequence);
    this.decodeStartedAt.set(d.capture_timestamp_us ?? d.sequence, performance.now());
    if (this.decodeStartedAt.size > 256) {
      const oldest = this.decodeStartedAt.keys().next().value;
      if (oldest !== undefined) this.decodeStartedAt.delete(oldest);
    }
    if (this.sequenceByTimestamp.size > 256) {
      const oldest = this.sequenceByTimestamp.keys().next().value;
      if (oldest !== undefined) this.sequenceByTimestamp.delete(oldest);
    }
    try {
      decoder.decode(
        new EncodedVideoChunk({
          type: d.keyframe ? "key" : "delta",
          timestamp: d.capture_timestamp_us ?? d.sequence,
          data: payload,
        }),
      );
    } catch {
      this.discardDecoder();
      this.requestKeyframe();
    }
  }

  private configureDecoder(epoch: number, keyframe: Uint8Array): boolean {
    this.discardDecoder();
    const codec = avcCodecFromAnnexB(keyframe) ?? "avc1.42E01F";
    const ctx = this.context;
    try {
      const decoder = new VideoDecoder({
        output: (frame) => {
          try {
            const canvas = this.options.canvas;
            if (canvas.width !== frame.displayWidth) canvas.width = frame.displayWidth;
            if (canvas.height !== frame.displayHeight) canvas.height = frame.displayHeight;
            ctx?.drawImage(frame, 0, 0);
            this.framesDecoded += 1;
            const started = this.decodeStartedAt.get(frame.timestamp);
            if (started !== undefined) {
              this.decodeStartedAt.delete(frame.timestamp);
              const ms = performance.now() - started;
              this.counters.decodeMs = this.counters.decodeMs === 0 ? ms : this.counters.decodeMs * 0.9 + ms * 0.1;
            }
            this.frameDrawn(frame.displayWidth, frame.displayHeight);
            const sequence = this.sequenceByTimestamp.get(frame.timestamp);
            if (sequence !== undefined) {
              this.sequenceByTimestamp.delete(frame.timestamp);
              this.send("frame_ack", {
                session_id: this.sessionId,
                sequence,
                decode_queue: this.decoder?.decodeQueueSize ?? 0,
              });
            }
          } finally {
            frame.close();
          }
        },
        error: () => {
          // A WebCodecs decoder that errored is closed for good: rebuild from
          // the next keyframe.
          this.discardDecoder();
          this.requestKeyframe();
        },
      });
      // Annex B: SPS/PPS are in-band, so no `description`.
      decoder.configure({ codec, optimizeForLatency: true });
      this.decoder = decoder;
      this.decoderEpoch = epoch;
      this.awaitingKeyframe = true;
      return true;
    } catch {
      this.discardDecoder();
      return false;
    }
  }

  private discardDecoder(): void {
    try {
      if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    } catch {
      // already closed
    }
    this.decoder = null;
    this.decoderEpoch = -1;
    this.sequenceByTimestamp.clear();
    this.decodeStartedAt.clear();
  }

  private requestKeyframe(): void {
    const now = performance.now();
    if (now - this.lastKeyframeRequest < KEYFRAME_RETRY_MS) return;
    this.lastKeyframeRequest = now;
    if (this.sessionId) this.send("request_keyframe", { session_id: this.sessionId });
  }

  // ---------------------------------------------------------------- input

  private flushInput(): void {
    if (!this.sessionId || this.opened?.policy === "view_only") {
      this.pending = [];
      return;
    }
    const events = coalesceText(this.pending);
    this.pending = [];
    for (let i = 0; i < events.length; i += MAX_INPUT_BATCH) {
      const batch = events.slice(i, i + MAX_INPUT_BATCH);
      this.send("interactive_input", {
        session_id: this.sessionId,
        first_sequence: this.sequencer.take(batch.length),
        events: batch,
      });
    }
  }

  private wireInput(): void {
    const canvas = this.options.canvas;
    canvas.tabIndex = 0;
    const on = <K extends keyof HTMLElementEventMap>(
      type: K,
      handler: (event: HTMLElementEventMap[K]) => void,
      options?: AddEventListenerOptions,
    ) => {
      canvas.addEventListener(type, handler as EventListener, options);
      this.detachInput.push(() => canvas.removeEventListener(type, handler as EventListener, options));
    };
    const point = (event: MouseEvent) => normalizePoint(event.clientX, event.clientY, canvas.getBoundingClientRect());
    on("pointermove", (event) => {
      const now = performance.now();
      if (now - this.lastMove < POINTER_MOVE_INTERVAL_MS) return;
      this.lastMove = now;
      this.queueInput({ kind: "pointer", phase: "move", button: null, ...point(event), modifiers: modifiersOf(event) });
    });
    on("pointerdown", (event) => {
      canvas.focus();
      this.queueInput({
        kind: "pointer",
        phase: "down",
        button: pointerButton(event.button),
        ...point(event),
        modifiers: modifiersOf(event),
      });
      event.preventDefault();
    });
    on("pointerup", (event) => {
      this.queueInput({
        kind: "pointer",
        phase: "up",
        button: pointerButton(event.button),
        ...point(event),
        modifiers: modifiersOf(event),
      });
    });
    on("contextmenu", (event) => event.preventDefault());
    on(
      "wheel",
      (event) => {
        event.preventDefault();
        this.queueInput({
          kind: "scroll",
          ...point(event),
          delta_x: event.deltaX,
          delta_y: event.deltaY,
          phase: "none",
          momentum_phase: "none",
          precise: event.deltaMode === 0,
        });
      },
      { passive: false },
    );
    on("keydown", (event) => {
      event.preventDefault();
      const input = keyEventToInput(event, "down");
      if (input) this.queueInput(input);
    });
    on("keyup", (event) => {
      event.preventDefault();
      const input = keyEventToInput(event, "up");
      if (input) this.queueInput(input);
    });
  }
}

function safeContext(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
  try {
    return canvas.getContext("2d", { alpha: false });
  } catch {
    return null;
  }
}

function closeReason(code: number): string {
  switch (code) {
    case 4404:
      return "the stream session was closed";
    case 4410:
      return "the window is gone";
    case 4429:
      return "too many viewers on this stream";
    default:
      return `closed (${code})`;
  }
}

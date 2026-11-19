// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Microphone uplink (MEDIA.md §12): the user's microphone into the guest's
 * virtual source. Capture runs in an AudioWorklet (`captureWorklet.js`),
 * frames are Opus-encoded with WebCodecs `AudioEncoder` (or sent as PCM
 * s16le when the engine has no encoder and the grant is PCM), and each
 * frame travels as one RAU2 packet on the media socket.
 *
 * The uplink track exists only when `OpenMedia` granted it (the viewer
 * ticket's `audio_uplink` and the guest's `InitRequest.audio_uplink`
 * policy). Mute is `audio_uplink_state{muted}` on the socket.
 */

import { encodeAudioPacket } from "./core/mediaWire";

export interface UplinkGrant {
  trackId: number;
  codec: "opus" | "pcm";
  sampleRate: number;
  channels: number;
  frameMs: number;
  bitrateKbps: number;
  configEpoch: number;
}

/** Interleaves float planes into s16le bytes. */
export function floatToS16le(planes: Float32Array[]): Uint8Array {
  const channels = planes.length;
  const frames = planes[0]?.length ?? 0;
  const out = new Uint8Array(frames * channels * 2);
  const view = new DataView(out.buffer);
  for (let i = 0; i < frames; i++) {
    for (let c = 0; c < channels; c++) {
      const v = Math.max(-1, Math.min(1, planes[c]![i] ?? 0));
      view.setInt16((i * channels + c) * 2, v < 0 ? v * 32768 : v * 32767, true);
    }
  }
  return out;
}

/** Linear resampling of one plane (the capture context runs at the
 * device rate; the uplink runs at the granted rate). */
export function resample(plane: Float32Array, from: number, to: number): Float32Array {
  if (from === to) return plane;
  const out = new Float32Array(Math.round((plane.length * to) / from));
  const step = from / to;
  for (let i = 0; i < out.length; i++) {
    const at = i * step;
    const j = Math.floor(at);
    const k = Math.min(j + 1, plane.length - 1);
    const t = at - j;
    out[i] = (plane[j] ?? 0) * (1 - t) + (plane[k] ?? 0) * t;
  }
  return out;
}

const CAPTURE_URL = new URL("./captureWorklet.js", import.meta.url);

export class MicUplink {
  private context: AudioContext | null = null;
  private stream: MediaStream | null = null;
  private node: AudioWorkletNode | null = null;
  private encoder: AudioEncoder | null = null;
  private sequence = Math.floor(Math.random() * 0xffff_ffff) >>> 0;
  private sent = 0;
  private readonly startedAt = performance.now();
  private captureRate = 48000;
  private captured = 0;
  private lastError: string | null = null;

  constructor(
    private grant: UplinkGrant,
    private readonly send: (packet: ArrayBuffer) => boolean,
    private readonly control: (type: string, payload: unknown) => void,
  ) {}

  /** Whether this engine can capture and send in the granted codec. */
  static supported(codec: "opus" | "pcm"): boolean {
    const capture =
      typeof navigator !== "undefined" &&
      Boolean(navigator.mediaDevices?.getUserMedia) &&
      typeof AudioWorkletNode !== "undefined";
    return capture && (codec === "pcm" || typeof AudioEncoder !== "undefined");
  }

  get packetsSent(): number {
    return this.sent;
  }

  /** Captured frames and the last encoder error (stats, tests). */
  get debug(): { captured: number; error: string | null; state: string } {
    return { captured: this.captured, error: this.lastError, state: this.context?.state ?? "closed" };
  }

  /** An `audio_config` for the uplink track changed the encoding. */
  reconfigure(grant: UplinkGrant): void {
    this.grant = grant;
    if (this.encoder) {
      this.encoder.close();
      this.encoder = null;
      this.makeEncoder();
    }
  }

  async start(): Promise<void> {
    const grant = this.grant;
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: grant.channels,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
    });
    // The device rate: some engines refuse to connect a MediaStream to a
    // context running at another rate. Frames are resampled to the grant.
    this.context = new AudioContext({ latencyHint: "interactive" });
    this.captureRate = this.context.sampleRate;
    await this.context.audioWorklet.addModule(CAPTURE_URL.href);
    const source = this.context.createMediaStreamSource(this.stream);
    const frameSamples = Math.round((this.captureRate * grant.frameMs) / 1000);
    this.node = new AudioWorkletNode(this.context, "cua-capture", {
      processorOptions: { frameSamples, channels: grant.channels },
      numberOfOutputs: 0,
    });
    this.node.port.onmessage = (event: MessageEvent<{ planes: Float32Array[] }>) => this.frame(event.data.planes);
    source.connect(this.node);
    // Started from a click, but some engines still create it suspended
    // (and never resume without an output device): do not wait on it.
    void this.context.resume().catch(() => {});
    if (grant.codec === "opus") this.makeEncoder();
    this.control("audio_uplink_state", { track_id: grant.trackId, muted: false });
  }

  stop(): void {
    this.control("audio_uplink_state", { track_id: this.grant.trackId, muted: true });
    this.node?.disconnect();
    this.node = null;
    for (const track of this.stream?.getTracks() ?? []) track.stop();
    this.stream = null;
    try {
      this.encoder?.close();
    } catch {
      // closed
    }
    this.encoder = null;
    void this.context?.close().catch(() => {});
    this.context = null;
  }

  private makeEncoder(): void {
    const grant = this.grant;
    const encoder = new AudioEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        const frameSamples = Math.round(((chunk.duration ?? grant.frameMs * 1000) * grant.sampleRate) / 1_000_000);
        this.packet(data, frameSamples);
      },
      error: (e) => {
        this.lastError = String(e?.message ?? e);
        this.encoder = null;
      },
    });
    const base = {
      codec: "opus",
      sampleRate: grant.sampleRate,
      numberOfChannels: grant.channels,
      bitrate: grant.bitrateKbps * 1000,
    };
    try {
      // One Opus packet per wire frame where the engine takes the hint.
      encoder.configure({ ...base, opus: { frameDuration: grant.frameMs * 1000 } } as AudioEncoderConfig);
    } catch {
      encoder.configure(base as AudioEncoderConfig);
    }
    this.encoder = encoder;
  }

  private frame(captured: Float32Array[]): void {
    const grant = this.grant;
    const planes = captured.map((p) => resample(p, this.captureRate, grant.sampleRate));
    const frameSamples = planes[0]?.length ?? 0;
    if (!frameSamples) return;
    this.captured += 1;
    if (grant.codec === "pcm") {
      this.packet(floatToS16le(planes), frameSamples);
      return;
    }
    if (!this.encoder || this.encoder.state !== "configured") return;
    const planar = new Float32Array(frameSamples * planes.length);
    for (let c = 0; c < planes.length; c++) planar.set(planes[c]!, c * frameSamples);
    const data = new AudioData({
      format: "f32-planar",
      sampleRate: grant.sampleRate,
      numberOfFrames: frameSamples,
      numberOfChannels: planes.length,
      timestamp: Math.round((performance.now() - this.startedAt) * 1000),
      data: planar,
    });
    try {
      this.encoder.encode(data);
    } finally {
      data.close();
    }
  }

  private packet(payload: Uint8Array, frameSamples: number): void {
    const packet = encodeAudioPacket(
      {
        trackId: this.grant.trackId,
        sequence: this.sequence,
        // The client's own monotonic clock (§12.5 uplink).
        ptsUs: Math.round(performance.now() * 1000),
        frameSamples,
        configEpoch: this.grant.configEpoch,
      },
      payload,
    );
    this.sequence = (this.sequence + 1) >>> 0;
    if (this.send(packet)) this.sent += 1;
  }
}

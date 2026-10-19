// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Downlink audio for one media session (rcdp wire v2 §12): WebCodecs
 * `AudioDecoder` for Opus (PCM s16le is decoded here directly), played
 * through an AudioWorklet jitter buffer (`audioWorklet.js`).
 *
 * Loss handling: a sequence gap is loss (DTX does not advance the sequence).
 * WebCodecs exposes neither Opus in-band FEC nor packet-loss concealment, so a
 * lost packet is concealed with silence of its duration, which keeps the
 * playout clock aligned. Late/duplicate packets are dropped, never played.
 *
 * Everything degrades to a no-op where WebCodecs/AudioWorklet are absent
 * (jsdom, older webviews): the video keeps streaming.
 */

import {
  classifyAudioSequence,
  JitterController,
  type AudioPacketHeader,
} from "./mediaWire";

/** `audio_config` payload (MEDIA.md §12.3). */
export interface AudioConfigMessage {
  track_id: number;
  config_epoch: number;
  direction: "down" | "up" | string;
  codec: string;
  sample_rate_hz: number;
  channels: number;
  frame_ms: number;
  bitrate_kbps?: number;
  fec?: boolean;
  dtx?: boolean;
  opus_pre_skip?: number;
}

/** Interleaved s16le → planar Float32. Returns null on a length mismatch
 * (the packet must be dropped, §12.2). */
export function pcmS16leToPlanes(
  payload: Uint8Array,
  channels: number,
  frameSamples: number,
): Float32Array[] | null {
  if (channels < 1) return null;
  if (payload.byteLength !== frameSamples * channels * 2) return null;
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const planes = Array.from({ length: channels }, () => new Float32Array(frameSamples));
  for (let frame = 0; frame < frameSamples; frame++) {
    for (let channel = 0; channel < channels; channel++) {
      planes[channel]![frame] = view.getInt16((frame * channels + channel) * 2, true) / 32768;
    }
  }
  return planes;
}

export interface AudioTrackStats {
  trackId: number;
  packets: number;
  lost: number;
  late: number;
  concealed: number;
  /** Frames decoded (Opus by AudioDecoder, or PCM) and handed to playout. */
  decoded: number;
  jitterMs: number;
  bufferTargetMs: number;
}

interface Track {
  config: AudioConfigMessage;
  decoder: AudioDecoder | null;
  node: AudioWorkletNode | null;
  jitter: JitterController;
  lastSequence: number | null;
  stats: AudioTrackStats;
}

const WORKLET_URL = new URL("./audioWorklet.js", import.meta.url);

export class AudioPlayer {
  private context: AudioContext | null = null;
  private workletReady: Promise<boolean> | null = null;
  private readonly tracks = new Map<number, Track>();
  private muted = false;
  private readonly unlock = () => void this.context?.resume().catch(() => {});

  /** Whether this environment can play wire audio at all. */
  static supported(): boolean {
    return (
      typeof AudioContext !== "undefined" &&
      typeof AudioWorkletNode !== "undefined" &&
      typeof window !== "undefined"
    );
  }

  setMuted(muted: boolean): void {
    this.muted = muted;
    if (this.context) void (muted ? this.context.suspend() : this.context.resume()).catch(() => {});
  }

  /** `audio_config`: (re)configure a downlink track, resetting its decoder
   * whenever the config epoch changes. */
  configure(config: AudioConfigMessage): void {
    if (config.direction !== "down") return;
    this.dropTrack(config.track_id);
    const track: Track = {
      config,
      decoder: null,
      node: null,
      jitter: new JitterController(Math.max(2.5, config.frame_ms || 20)),
      lastSequence: null,
      stats: {
        trackId: config.track_id,
        packets: 0,
        lost: 0,
        late: 0,
        concealed: 0,
        decoded: 0,
        jitterMs: 0,
        bufferTargetMs: 0,
      },
    };
    this.tracks.set(config.track_id, track);
    if (!AudioPlayer.supported()) return;
    void this.ensureNode(track);
    if (config.codec === "opus" && typeof AudioDecoder !== "undefined") {
      try {
        const decoder = new AudioDecoder({
          output: (data) => {
            try {
              const planes: Float32Array[] = [];
              for (let c = 0; c < data.numberOfChannels; c++) {
                const plane = new Float32Array(data.numberOfFrames);
                data.copyTo(plane, { planeIndex: c, format: "f32-planar" });
                planes.push(plane);
              }
              track.stats.decoded += 1;
              this.play(track, planes);
            } finally {
              data.close();
            }
          },
          error: () => {
            // A closed decoder never recovers; the next audio_config rebuilds.
            track.decoder = null;
          },
        });
        decoder.configure({
          codec: "opus",
          sampleRate: config.sample_rate_hz,
          numberOfChannels: config.channels,
        });
        track.decoder = decoder;
      } catch {
        track.decoder = null;
      }
    }
  }

  /** `audio_track_state`: silence lets the jitter target shrink; `ended`
   * drops the track. */
  trackState(trackId: number, state: string): void {
    const track = this.tracks.get(trackId);
    if (!track) return;
    if (state === "ended") this.dropTrack(trackId);
    else if (state === "silent" || state === "paused") track.jitter.onSilence(performance.now());
  }

  /** One RAU2 packet. */
  packet(header: AudioPacketHeader, payload: Uint8Array): void {
    const track = this.tracks.get(header.trackId);
    if (!track) return; // unknown track: drop (§12.2)
    if (header.configEpoch !== (track.config.config_epoch & 0xff)) return; // stale epoch
    const verdict = classifyAudioSequence(track.lastSequence, header.sequence);
    if (verdict.kind === "late") {
      track.stats.late += 1;
      return;
    }
    track.lastSequence = header.sequence;
    track.stats.packets += 1;
    track.jitter.onPacket(header.ptsUs, performance.now());
    if (verdict.kind === "gap") {
      track.stats.lost += verdict.lost;
      // Conceal each lost packet with silence of its duration (no WebCodecs PLC).
      const lostFrames = Math.min(verdict.lost, 10) * header.frameSamples;
      if (lostFrames > 0) {
        track.stats.concealed += Math.min(verdict.lost, 10);
        this.play(
          track,
          Array.from({ length: track.config.channels }, () => new Float32Array(lostFrames)),
        );
      }
    }
    if (header.dtx) track.jitter.onSilence(performance.now());
    this.pushTarget(track);
    if (track.config.codec === "opus") {
      if (!track.decoder || typeof EncodedAudioChunk === "undefined") return;
      try {
        track.decoder.decode(
          new EncodedAudioChunk({ type: "key", timestamp: header.ptsUs, data: payload }),
        );
      } catch {
        track.decoder = null;
      }
    } else {
      const planes = pcmS16leToPlanes(payload, track.config.channels, header.frameSamples);
      if (planes) {
        track.stats.decoded += 1;
        this.play(track, planes);
      }
    }
  }

  stats(): AudioTrackStats[] {
    return [...this.tracks.values()].map((t) => ({
      ...t.stats,
      jitterMs: t.jitter.jitter,
      bufferTargetMs: t.jitter.target,
    }));
  }

  close(): void {
    for (const id of [...this.tracks.keys()]) this.dropTrack(id);
    if (typeof window !== "undefined") window.removeEventListener("pointerdown", this.unlock);
    void this.context?.close().catch(() => {});
    this.context = null;
    this.workletReady = null;
  }

  private play(track: Track, planes: Float32Array[]): void {
    if (this.muted || !track.node) return;
    track.node.port.postMessage({ type: "push", planes });
  }

  private pushTarget(track: Track): void {
    if (!track.node) return;
    const samples = Math.round((track.jitter.target / 1000) * track.config.sample_rate_hz);
    track.node.port.postMessage({ type: "target", samples });
  }

  private async ensureNode(track: Track): Promise<void> {
    if (!this.context) {
      try {
        this.context = new AudioContext({
          sampleRate: track.config.sample_rate_hz || 48_000,
          latencyHint: "interactive",
        });
      } catch {
        return;
      }
      // Autoplay policy: resume on the first user gesture if needed.
      window.addEventListener("pointerdown", this.unlock);
      this.workletReady = this.context.audioWorklet
        .addModule(WORKLET_URL.href)
        .then(() => true)
        .catch(() => false);
    }
    const ok = await this.workletReady;
    if (!ok || !this.context || this.tracks.get(track.config.track_id) !== track) return;
    const node = new AudioWorkletNode(this.context, "cua-playout", {
      outputChannelCount: [Math.max(1, track.config.channels)],
    });
    node.port.onmessage = (event: MessageEvent<{ type?: string }>) => {
      if (event.data?.type === "underrun") {
        track.jitter.onUnderrun();
        this.pushTarget(track);
      }
    };
    node.port.postMessage({ type: "config", channels: track.config.channels });
    node.connect(this.context.destination);
    track.node = node;
    this.pushTarget(track);
    if (!this.muted) void this.context.resume().catch(() => {});
  }

  private dropTrack(trackId: number): void {
    const track = this.tracks.get(trackId);
    if (!track) return;
    try {
      track.decoder?.close();
    } catch {
      // already closed
    }
    try {
      track.node?.disconnect();
    } catch {
      // already gone
    }
    this.tracks.delete(trackId);
  }
}

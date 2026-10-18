// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Cua Spaces audio playout worklet (rcdp wire v2 audio, MEDIA.md §12.5).
//
// The main thread decodes (WebCodecs AudioDecoder for Opus, or raw PCM) and
// posts planar Float32 chunks here. This processor is only the jitter buffer:
// a bounded ring per channel that primes to the target depth before playing,
// reports underruns (the main thread then grows the target), and drops the
// oldest samples when the backlog exceeds target + 200 ms so late audio is
// never played late.
class CuaPlayout extends AudioWorkletProcessor {
  constructor() {
    super();
    this.channels = 2;
    this.capacity = sampleRate * 2; // 2 s ring, a hard bound
    this.rings = [new Float32Array(this.capacity), new Float32Array(this.capacity)];
    this.read = 0;
    this.write = 0;
    this.buffered = 0;
    this.target = Math.round(sampleRate * 0.04);
    this.priming = true;
    this.port.onmessage = (event) => this.onMessage(event.data);
  }

  onMessage(message) {
    if (!message || typeof message !== "object") return;
    if (message.type === "config") {
      this.channels = Math.max(1, Math.min(8, message.channels | 0));
      this.rings = Array.from({ length: this.channels }, () => new Float32Array(this.capacity));
      this.read = this.write = this.buffered = 0;
      this.priming = true;
    } else if (message.type === "target") {
      this.target = Math.max(1, Math.min(this.capacity / 2, message.samples | 0));
    } else if (message.type === "flush") {
      this.read = this.write = this.buffered = 0;
      this.priming = true;
    } else if (message.type === "push") {
      this.push(message.planes);
    }
  }

  push(planes) {
    if (!Array.isArray(planes) || planes.length === 0) return;
    const frames = planes[0].length;
    const max = Math.min(this.capacity, this.target + Math.round(sampleRate * 0.2));
    // Late backlog: drop the oldest so playout stays near the target.
    const overflow = this.buffered + frames - max;
    if (overflow > 0) {
      const drop = Math.min(overflow, this.buffered);
      this.read = (this.read + drop) % this.capacity;
      this.buffered -= drop;
    }
    const take = Math.min(frames, this.capacity - this.buffered);
    for (let c = 0; c < this.channels; c++) {
      const src = planes[Math.min(c, planes.length - 1)];
      const ring = this.rings[c];
      let w = this.write;
      for (let i = 0; i < take; i++) {
        ring[w] = src[i];
        w = w + 1 === this.capacity ? 0 : w + 1;
      }
    }
    this.write = (this.write + take) % this.capacity;
    this.buffered += take;
  }

  process(_inputs, outputs) {
    const output = outputs[0];
    if (!output || output.length === 0) return true;
    const frames = output[0].length;
    if (this.priming) {
      if (this.buffered < this.target) {
        for (const channel of output) channel.fill(0);
        return true;
      }
      this.priming = false;
    }
    const take = Math.min(frames, this.buffered);
    for (let c = 0; c < output.length; c++) {
      const ring = this.rings[Math.min(c, this.channels - 1)];
      const out = output[c];
      let r = this.read;
      for (let i = 0; i < take; i++) {
        out[i] = ring[r];
        r = r + 1 === this.capacity ? 0 : r + 1;
      }
      for (let i = take; i < frames; i++) out[i] = 0;
    }
    this.read = (this.read + take) % this.capacity;
    this.buffered -= take;
    if (take < frames) {
      this.priming = true;
      this.port.postMessage({ type: "underrun", buffered: this.buffered });
    }
    return true;
  }
}

registerProcessor("cua-playout", CuaPlayout);

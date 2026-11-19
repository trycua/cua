// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Microphone capture for the uplink: collects `frameSamples` per channel and
// posts one planar Float32 frame at a time to the main thread.
class CuaCapture extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { frameSamples, channels } = options.processorOptions;
    this.frameSamples = frameSamples;
    this.channels = channels;
    this.planes = Array.from({ length: channels }, () => new Float32Array(frameSamples));
    this.fill = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) return true;
    const frames = input[0].length;
    let offset = 0;
    while (offset < frames) {
      const take = Math.min(frames - offset, this.frameSamples - this.fill);
      for (let c = 0; c < this.channels; c++) {
        const source = input[Math.min(c, input.length - 1)];
        this.planes[c].set(source.subarray(offset, offset + take), this.fill);
      }
      this.fill += take;
      offset += take;
      if (this.fill === this.frameSamples) {
        const planes = this.planes;
        this.port.postMessage({ planes }, planes.map((p) => p.buffer));
        this.planes = Array.from({ length: this.channels }, () => new Float32Array(this.frameSamples));
        this.fill = 0;
      }
    }
    return true;
  }
}

registerProcessor("cua-capture", CuaCapture);

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { AudioPlayer, pcmS16leToPlanes } from "./audio";

describe("pcmS16leToPlanes", () => {
  it("de-interleaves s16le into planar floats", () => {
    // Two frames, stereo: (L=16384, R=-32768), (L=0, R=32767).
    const bytes = new Uint8Array(8);
    const view = new DataView(bytes.buffer);
    view.setInt16(0, 16384, true);
    view.setInt16(2, -32768, true);
    view.setInt16(4, 0, true);
    view.setInt16(6, 32767, true);
    const planes = pcmS16leToPlanes(bytes, 2, 2)!;
    expect(planes).toHaveLength(2);
    expect([...planes[0]!]).toEqual([0.5, 0]);
    expect(planes[1]![0]).toBe(-1);
    expect(planes[1]![1]).toBeCloseTo(32767 / 32768);
  });

  it("drops a payload whose length does not match (§12.2)", () => {
    expect(pcmS16leToPlanes(new Uint8Array(6), 2, 2)).toBeNull();
    expect(pcmS16leToPlanes(new Uint8Array(0), 0, 0)).toBeNull();
  });
});

describe("AudioPlayer without WebAudio (jsdom)", () => {
  it("counts packets, loss and late packets per track and never throws", () => {
    const player = new AudioPlayer();
    player.configure({
      track_id: 3,
      config_epoch: 1,
      direction: "down",
      codec: "pcm_s16le",
      sample_rate_hz: 48_000,
      channels: 1,
      frame_ms: 20,
    });
    const header = (sequence: number, configEpoch = 1) => ({
      flags: 0,
      discontinuity: false,
      dtx: false,
      trackId: 3,
      sequence,
      ptsUs: sequence * 20_000,
      frameSamples: 960,
      configEpoch,
    });
    const pcm = new Uint8Array(960 * 2);
    player.packet(header(10), pcm);
    player.packet(header(11), pcm);
    player.packet(header(14), pcm); // 2 lost
    player.packet(header(12), pcm); // late: dropped
    player.packet(header(15, 0), pcm); // stale config epoch: dropped
    player.packet({ ...header(16), trackId: 9 }, pcm); // unknown track: dropped
    const [stats] = player.stats();
    expect(stats).toMatchObject({ trackId: 3, packets: 3, lost: 2, late: 1, concealed: 2 });
    expect(stats!.bufferTargetMs).toBeGreaterThanOrEqual(20);
    player.close();
  });

  it("ignores uplink configs (the downlink player only plays down tracks)", () => {
    const player = new AudioPlayer();
    player.configure({
      track_id: 2,
      config_epoch: 0,
      direction: "up",
      codec: "opus",
      sample_rate_hz: 48_000,
      channels: 1,
      frame_ms: 20,
    });
    expect(player.stats()).toEqual([]);
  });
});

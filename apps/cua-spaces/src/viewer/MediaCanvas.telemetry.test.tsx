// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { TelemetryBridge } from "../native/telemetry";

// A stand-in session: no socket, fixed counters, one geometry callback.
vi.mock("@cua/spacesd-html5/core/mediaSession", () => {
  class MediaSession {
    constructor(private readonly options: { onGeometry?: (w: number, h: number) => void }) {}
    start() {
      this.options.onGeometry?.(1920, 1080);
    }
    stop() {}
    get stats() {
      return { framesReceived: 240, framesDecoded: 240, codec: "h264" };
    }
  }
  return { MediaSession };
});

const { MediaCanvas } = await import("./WindowStream");

function bridge() {
  const recordStream = vi.fn();
  const t: TelemetryBridge = {
    isNative: true,
    status: vi.fn(),
    setEnabled: vi.fn(),
    acknowledgeNotice: vi.fn(),
    recordFeature: vi.fn(),
    recordStep: vi.fn(),
    recordStream,
    recordSignals: vi.fn(),
    welcomeLeft: vi.fn(),
  } as unknown as TelemetryBridge;
  return { t, recordStream };
}

describe("MediaCanvas stream telemetry", () => {
  it("reports one raw summary to the native bucketer when the stream ends", () => {
    const { t, recordStream } = bridge();
    const view = render(
      <MediaCanvas ticket={{ wsUrl: "ws://127.0.0.1:1/media?ticket=x" }} interactive={false} audio={false} telemetry={t} />,
    );
    expect(recordStream).not.toHaveBeenCalled();
    view.unmount();
    expect(recordStream).toHaveBeenCalledTimes(1);
    const report = recordStream.mock.calls[0]![0];
    // Only these fields cross to the shell, which buckets them; no URL,
    // ticket, session id or title.
    expect(Object.keys(report).sort()).toEqual(["codec", "durationMs", "frames", "height"]);
    expect(report).toMatchObject({ codec: "h264", frames: 240, height: 1080 });
    expect(report.durationMs).toBeGreaterThanOrEqual(0);
  });

  it("reports nothing when no session was started", () => {
    const { t, recordStream } = bridge();
    const view = render(<MediaCanvas interactive={false} audio={false} telemetry={t} />);
    view.unmount();
    expect(recordStream).not.toHaveBeenCalled();
  });
});

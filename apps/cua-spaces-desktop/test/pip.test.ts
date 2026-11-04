// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The picture-in-picture panel's place, shape and page (pip-layout.ts): the
// SwiftUI panel's 480 pt width and stream-locked shape, where it opens, and
// the web UI's view it loads.
import { describe, expect, it } from "vitest";
import { aspectOf, fitToStream, initialPipBounds, onScreen, pipPath, PIP_CASCADE, PIP_DEFAULT_ASPECT } from "../src/pip-layout";
import type { PipSpec } from "../src/model/streams";

const work = { x: 0, y: 25, width: 1440, height: 875 };

describe("the picture-in-picture panel", () => {
  it("opens 480 wide at 16:10, centred, until a place was saved", () => {
    expect(initialPipBounds({ saved: null, work, areas: [work], aspect: PIP_DEFAULT_ASPECT, open: 0 })).toEqual({ x: 480, y: 313, width: 480, height: 300 });
  });

  it("opens where the last one was left, and cascades beside an open one", () => {
    const saved = { x: 900, y: 600, width: 360, height: 225 };
    expect(initialPipBounds({ saved, work, areas: [work], aspect: 16 / 9, open: 0 })).toEqual({ x: 900, y: 600, width: 360, height: 203 });
    expect(initialPipBounds({ saved, work, areas: [work], aspect: 16 / 10, open: 2 })).toMatchObject({ x: 900 + 2 * PIP_CASCADE, y: 600 + 2 * PIP_CASCADE });
  });

  it("forgets a place that is off every display", () => {
    const saved = { x: 4000, y: 600, width: 360, height: 225 };
    expect(onScreen(saved, [work])).toBe(false);
    expect(initialPipBounds({ saved, work, areas: [work], aspect: PIP_DEFAULT_ASPECT, open: 0 })).toMatchObject({ width: 360, x: 540 });
  });

  it("takes the stream's shape once its size is known, keeping the width", () => {
    expect(fitToStream({ x: 10, y: 20, width: 480, height: 300 }, 800, 1200)).toEqual({ x: 10, y: 20, width: 480, height: 720 });
    expect(aspectOf(0, 0)).toBe(PIP_DEFAULT_ASPECT);
  });

  it("loads the web UI's picture-in-picture view for the Space and the window", () => {
    const spec: PipSpec = {
      spaceId: "local:aurora",
      spaceName: "Aurora",
      os: "linux",
      key: "window:w-1",
      title: "cua@space: ~",
      source: { kind: "window", window: { id: "w-1", app: "Terminal", title: "cua@space: ~", epoch: 3, width: 0, height: 0, appId: "", pid: 0 } },
    };
    const url = new URL(`cua-spaces://app${pipPath(spec)}`);
    expect(url.pathname).toBe("/pip");
    expect(Object.fromEntries(url.searchParams)).toEqual({ space: "local:aurora", name: "Aurora", title: "cua@space: ~", key: "window:w-1", os: "linux", window: "w-1", epoch: "3" });
    const desktop = new URL(`cua-spaces://app${pipPath({ ...spec, key: "desktop", title: "Desktop", source: { kind: "desktop" } })}`);
    expect(desktop.searchParams.has("window")).toBe(false);
  });
});

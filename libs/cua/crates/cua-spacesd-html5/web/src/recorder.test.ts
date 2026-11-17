// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { InteractiveInputEvent } from "./core/mediaWire";
import { ActionLog, frameRecording } from "./recorder";

describe("skill recording", () => {
  it("turns input into click, double click, drag, type and key steps", () => {
    const log = new ActionLog(0, () => ({ width: 1000, height: 500 }));
    const p = (phase: "down" | "up", x: number, y: number, button: "left" | "right" = "left"): InteractiveInputEvent => ({
      kind: "pointer",
      phase,
      button,
      x_normalized: x,
      y_normalized: y,
      modifiers: [],
    });
    log.add(p("down", 0.1, 0.1), 100);
    log.add(p("up", 0.1, 0.1), 150);
    log.add(p("down", 0.1, 0.1), 300);
    log.add(p("up", 0.1, 0.1), 350);
    log.add(p("down", 0.2, 0.2), 1000);
    log.add(p("up", 0.5, 0.5), 1200);
    log.add(p("down", 0.3, 0.3, "right"), 1500);
    log.add(p("up", 0.3, 0.3, "right"), 1550);
    log.add({ kind: "text_commit", text: "he" }, 2000);
    log.add({ kind: "text_commit", text: "llo" }, 2100);
    log.add({ kind: "key", key: "enter", state: "down", modifiers: [], repeat: false }, 2200);
    expect(log.events.map((e) => e.type)).toEqual(["double_click", "drag", "right_click", "type", "key"]);
    expect(log.events[0]).toMatchObject({ x: 100, y: 50, timestamp: 100 });
    expect(log.events[1]).toMatchObject({ x: 200, y: 100, to_x: 500, to_y: 250 });
    expect(log.events[3]!.text).toBe("hello");
  });

  it("frames [u32 BE length][json][video]", () => {
    const framed = frameRecording({ events: [] }, new Uint8Array([1, 2, 3]));
    const n = new DataView(framed.buffer).getUint32(0, false);
    expect(JSON.parse(new TextDecoder().decode(framed.slice(4, 4 + n)))).toEqual({ events: [] });
    expect([...framed.slice(4 + n)]).toEqual([1, 2, 3]);
  });
});

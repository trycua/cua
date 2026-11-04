// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  applyDragOverlay,
  initialDragOverlay,
  type DragOverlayEvent,
  type DragOverlayState,
} from "./dragOverlay";

function run(events: DragOverlayEvent[], start: DragOverlayState = initialDragOverlay) {
  let state = start;
  const effects = [] as ReturnType<typeof applyDragOverlay>["effects"];
  for (const event of events) {
    const next = applyDragOverlay(state, event);
    state = next.state;
    effects.push(...next.effects);
  }
  return { state, effects };
}

describe("drag overlay state machine", () => {
  it("shows the prompt on start and captures the ghost (never touches the window)", () => {
    const { state, effects } = run([{ type: "start", windowId: 7, appName: "Google Chrome" }]);
    expect(state.phase).toBe("prompt");
    expect(state.windowId).toBe(7);
    expect(state.appName).toBe("Google Chrome");
    // The only effect is the additive capture; nothing hides/moves the window.
    expect(effects).toEqual([{ kind: "capture", windowId: 7 }]);
  });

  it("stores the captured ghost while still in the prompt", () => {
    const { state, effects } = run([
      { type: "start", windowId: 7, appName: "Chrome" },
      { type: "ghost-ready", ghost: "data:image/png;base64,ZZZ" },
    ]);
    expect(state.phase).toBe("prompt");
    expect(state.ghost).toBe("data:image/png;base64,ZZZ");
    expect(effects).toEqual([{ kind: "capture", windowId: 7 }]);
  });

  it("prompt ⟷ selector are mutually exclusive: enter-notch expands, leave-notch collapses", () => {
    // start → prompt (selector not yet open)
    let state = applyDragOverlay(initialDragOverlay, {
      type: "start",
      windowId: 7,
      appName: "Chrome",
    }).state;
    expect(state.phase).toBe("prompt");

    // Bring the window up to the notch → selector expands, prompt is gone.
    state = applyDragOverlay(state, { type: "enter-notch" }).state;
    expect(state.phase).toBe("selector");

    // Only the selector has tiles, so `over` retargets only here.
    state = applyDragOverlay(state, { type: "over", spaceId: "space-a" }).state;
    expect(state.targetSpaceId).toBe("space-a");

    // Pull back away from the notch → collapse to the prompt, target cleared.
    state = applyDragOverlay(state, { type: "leave-notch" }).state;
    expect(state.phase).toBe("prompt");
    expect(state.targetSpaceId).toBeNull();

    // The phase is a single enum, so the two surfaces can never both be active.
    expect(["idle", "prompt", "selector"]).toContain(state.phase);
  });

  it("ignores tile targeting while only the prompt is up (no selector = no tiles)", () => {
    const { state } = run([
      { type: "start", windowId: 7, appName: "Chrome" },
      { type: "over", spaceId: "space-a" },
    ]);
    expect(state.phase).toBe("prompt");
    expect(state.targetSpaceId).toBeNull();
  });

  it("retargets across tiles inside the selector without extra effects", () => {
    const { state, effects } = run([
      { type: "start", windowId: 7, appName: "Chrome" },
      { type: "enter-notch" },
      { type: "over", spaceId: "space-a" },
      { type: "over", spaceId: "space-b" },
    ]);
    expect(state.targetSpaceId).toBe("space-b");
    // Only the initial capture; retargeting produces no side effects.
    expect(effects).toEqual([{ kind: "capture", windowId: 7 }]);
  });

  it("drop over a tile in the selector commits the teleport (and never restores a window)", () => {
    const { state, effects } = run([
      { type: "start", windowId: 7, appName: "Chrome" },
      { type: "enter-notch" },
      { type: "over", spaceId: "space-a" },
      { type: "drop", spaceId: "space-a" },
    ]);
    expect(state).toEqual(initialDragOverlay);
    expect(effects).toEqual([
      { kind: "capture", windowId: 7 },
      { kind: "commit", spaceId: "space-a" },
    ]);
    // Nothing in the flow ever hides/restores/moves the real window.
    expect(effects.some((e) => e.kind !== "capture" && e.kind !== "commit")).toBe(false);
  });

  it("drop while only the prompt is up (away from the notch) dismisses without committing", () => {
    const { state, effects } = run([
      { type: "start", windowId: 7, appName: "Chrome" },
      { type: "drop", spaceId: null },
    ]);
    expect(state).toEqual(initialDragOverlay);
    expect(effects.some((e) => e.kind === "commit")).toBe(false);
  });

  it("cancel dismisses both surfaces with no effects", () => {
    const { state, effects } = run([
      { type: "start", windowId: 7, appName: "Chrome" },
      { type: "enter-notch" },
      { type: "cancel" },
    ]);
    expect(state).toEqual(initialDragOverlay);
    expect(effects).toEqual([{ kind: "capture", windowId: 7 }]);
  });

  it("ignores notch/tile events while idle", () => {
    expect(run([{ type: "enter-notch" }]).state).toEqual(initialDragOverlay);
    expect(run([{ type: "leave-notch" }]).state).toEqual(initialDragOverlay);
    expect(run([{ type: "over", spaceId: "x" }]).state).toEqual(initialDragOverlay);
    expect(run([{ type: "out" }]).state).toEqual(initialDragOverlay);
    expect(run([{ type: "ghost-ready", ghost: "x" }]).state).toEqual(initialDragOverlay);
  });
});

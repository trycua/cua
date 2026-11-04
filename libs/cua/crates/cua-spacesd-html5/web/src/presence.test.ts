// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { create } from "@bufbuild/protobuf";
import { CURSOR_SHAPES } from "@trycua/cua/spaces/presence";
import { describe, expect, it } from "vitest";

import type { Api } from "./api";
import { renderCursor } from "./core/presenceLayer";
import { PrincipalKind } from "./gen/cua/env/v1/common_pb";
import {
  CursorShape,
  CursorShapeSource,
  JoinResponseSchema,
  LeaveReason,
  type JoinResponse,
} from "./gen/cua/env/v1/presence_pb";
import { presenceEvents, ViewerPresence } from "./presence";

const principal = (id: string, name: string, color: string, kind = PrincipalKind.HUMAN) => ({ id, displayName: name, color, kind });
const participant = (id: string, name: string, color: string, kind = PrincipalKind.HUMAN) => ({ participantId: id, principal: principal(`user:${id}`, name, color, kind) });

function msg(event: JoinResponse["event"]): JoinResponse {
  return create(JoinResponseSchema, { event } as never);
}

const moved = (id: string, x: number, y: number, shape = CursorShape.UNSPECIFIED, seconds = 0n) => ({
  participantId: id,
  cursor: { displayId: "", position: { x, y }, visible: true, shape, shapeSource: shape ? CursorShapeSource.HIT_TEST : CursorShapeSource.UNSPECIFIED },
  ...(seconds ? { at: { seconds, nanos: 0 } } : {}),
});

describe("presenceEvents", () => {
  it("maps the Join stream onto the SDK's events", () => {
    expect(presenceEvents(msg({ case: "participantJoined", value: participant("a", "Ada", "#e6194b", PrincipalKind.AGENT) } as never), 5)).toEqual([
      { kind: "joined", participant: { participantId: "a", principalId: "user:a", displayName: "Ada", color: "#e6194b", kind: "agent" } },
    ]);
    const batch = presenceEvents(
      msg({ case: "cursorBatch", value: { moves: [moved("a", 0.1, 0.2, CursorShape.TEXT, 2n), moved("b", 0.3, 0.4)], tick: 7n } } as never),
      9,
    );
    expect(batch).toHaveLength(2);
    expect(batch[0]).toEqual({
      kind: "cursor_moved",
      participantId: "a",
      cursor: { displayId: "", x: 0.1, y: 0.2, visible: true, pressed: false, shape: "text", shapeSource: "hit_test", atMs: 2000, receivedMs: 9 },
    });
    expect(presenceEvents(msg({ case: "participantLeft", value: { participantId: "a", reason: LeaveReason.RUN_ENDED } } as never), 0)).toEqual([
      { kind: "left", participantId: "a", reason: "run_ended" },
    ]);
    expect(
      presenceEvents(msg({ case: "cursorShapeChanged", value: { participantId: "me", shape: CursorShape.RESIZE_NWSE, source: CursorShapeSource.PROBE } } as never), 0),
    ).toEqual([{ kind: "shape_changed", participantId: "me", shape: "resize_nwse", shapeSource: "probe" }]);
    expect(presenceEvents(msg({ case: "rosterHeartbeat", value: { participantIds: ["me", "a"] } } as never), 0)).toEqual([
      { kind: "heartbeat", participantIds: ["me", "a"] },
    ]);
  });
});

/** A fake gRPC-Web presence client whose Join stream the test feeds. */
function fakeApi() {
  const queue: JoinResponse[] = [];
  let wake: (() => void) | null = null;
  const updates: unknown[] = [];
  const leaves: string[] = [];
  const push = (m: JoinResponse) => {
    queue.push(m);
    wake?.();
  };
  const api = {
    presence: {
      join: (_req: unknown, opts: { signal: AbortSignal }) =>
        (async function* () {
          while (!opts.signal.aborted) {
            const next = queue.shift();
            if (next) {
              yield next;
              continue;
            }
            await new Promise<void>((r) => {
              wake = r;
              opts.signal.addEventListener("abort", () => r(), { once: true });
            });
          }
        })(),
      updateCursor: async (req: unknown) => {
        updates.push(req);
        return {};
      },
      leave: async (req: { participantId: string }) => {
        leaves.push(req.participantId);
        return {};
      },
    },
  } as unknown as Api;
  return { api, push, updates, leaves };
}

async function settle() {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
}

function stage() {
  const host = document.createElement("div");
  const surface = document.createElement("canvas");
  host.appendChild(surface);
  document.body.appendChild(host);
  surface.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1000, height: 500, right: 1000, bottom: 500, x: 0, y: 0, toJSON: () => ({}) });
  return { host, surface };
}

const move = (el: Element, type: string, clientX: number, clientY: number) => el.dispatchEvent(new MouseEvent(type, { clientX, clientY, bubbles: true }));

describe("ViewerPresence", () => {
  it("draws my cursor locally, others from the stream, and drops them on leave and missed heartbeats", async () => {
    const f = fakeApi();
    const { host, surface } = stage();
    let clock = 1_000_000;
    const p = new ViewerPresence(f.api, surface, host, true, "primary", () => clock);
    void p.start();
    f.push(msg({ case: "joined", value: { participant: participant("me", "Me", "#3cb44b"), roster: [] } } as never));
    await settle();

    // Mine: at the pointer at once, system cursor hidden over the video only.
    move(surface, "pointermove", 250, 100);
    const own = host.querySelector(".presence-cursor-me") as HTMLElement;
    expect(own.style.transform).toBe("translate(250px, 100px)");
    expect(own.innerHTML).toContain('fill="#3cb44b"');
    expect(surface.style.cursor).toBe("none");
    expect(host.style.cursor).toBe("");
    expect(f.updates[0]).toEqual({ participantId: "me", cursor: { displayId: "", position: { x: 0.25, y: 0.2 }, visible: true } });

    // My shape comes from the server.
    f.push(msg({ case: "cursorShapeChanged", value: { participantId: "me", shape: CursorShape.TEXT, source: CursorShapeSource.SYSTEM } } as never));
    await settle();
    move(surface, "pointermove", 260, 100);
    expect(own.dataset.shape).toBe("text");

    // Others.
    f.push(msg({ case: "participantJoined", value: participant("agent", "Claude", "#4363d8", PrincipalKind.AGENT) } as never));
    f.push(msg({ case: "participantJoined", value: participant("dana", "Dana", "#e6194b") } as never));
    f.push(msg({ case: "cursorBatch", value: { moves: [moved("agent", 0.5, 0.5, CursorShape.POINTER), moved("dana", 0.1, 0.1)], tick: 1n } } as never));
    await settle();
    const layerRender = () => (host.querySelector(".presence-layer") as HTMLElement) && (p as unknown as { layer: { render(t: number): void } }).layer.render(clock);
    clock += 200;
    layerRender();
    const agent = host.querySelector('[data-participant="agent"]') as HTMLElement;
    expect(agent.dataset.shape).toBe("pointer");
    expect(agent.style.transform).toBe("translate(500px, 250px)");

    // Run end removes the agent.
    f.push(msg({ case: "participantLeft", value: { participantId: "agent", reason: LeaveReason.RUN_ENDED } } as never));
    await settle();
    layerRender();
    expect(host.querySelector('[data-participant="agent"]')).toBeNull();

    // Idle fade (Dana has not moved for 5.15 s).
    clock += 4_950;
    layerRender();
    expect(Number((host.querySelector('[data-participant="dana"]') as HTMLElement).style.opacity)).toBeCloseTo(0.5, 2);

    // A heartbeat without Dana removes her.
    f.push(msg({ case: "cursorMoved", value: moved("dana", 0.2, 0.1) } as never));
    f.push(msg({ case: "rosterHeartbeat", value: { participantIds: ["me"] } } as never));
    await settle();
    layerRender();
    expect(host.querySelector('[data-participant="dana"]')).toBeNull();

    // Leaving the stream shows the system cursor again; stop leaves presence.
    move(surface, "pointerleave", 0, 0);
    expect(surface.style.cursor).toBe("");
    p.stop();
    expect(f.leaves).toEqual(["me"]);
    expect(host.querySelector(".presence-layer")).toBeNull();
  });
});

describe("shaped, colored cursors", () => {
  for (const color of ["#e6194b", "#4363d8"]) {
    it(`renders every shape in ${color}`, () => {
      const out: Record<string, string> = {};
      for (const shape of CURSOR_SHAPES) {
        const el = document.createElement("div");
        renderCursor(el, shape, color, shape === "pointer" ? "Dana" : null);
        out[shape] = el.innerHTML;
      }
      expect(out).toMatchSnapshot();
    });
  }
});

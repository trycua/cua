// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { CURSOR_SHAPES } from "@trycua/cua/spaces/presence";
import { renderCursor, type PresenceLayer } from "@cua/spacesd-html5/core/presenceLayer";

import type { PresenceBridge, PresenceEvent, PresencePointer } from "../../native/presence";
import { PresenceOverlay } from "./PresenceOverlay";

const ME = { participantId: "me", principalId: "user:me", displayName: "Me", color: "#3cb44b", kind: "human" };
const DANA = { participantId: "dana", principalId: "user:dana", displayName: "Dana", color: "#e6194b", kind: "human" };
const AGENT = { participantId: "agent-1", principalId: "agent:run", displayName: "Claude", color: "#4363d8", kind: "agent" };

/** A pointer event with coordinates (jsdom has no PointerEvent constructor). */
function pointer(el: Element, type: string, clientX: number, clientY: number) {
  act(() => {
    el.dispatchEvent(new MouseEvent(type, { clientX, clientY, bubbles: true }));
  });
}

function fakeBridge() {
  let handler: ((e: PresenceEvent) => void) | undefined;
  const published: PresencePointer[] = [];
  let publishResolved = false;
  const bridge: PresenceBridge = {
    join: async () => ({ handle: "h1", me: ME, members: [], delayMs: 100, datagrams: false }),
    onEvent: async (_h, fn) => {
      handler = fn;
      return () => {
        handler = undefined;
      };
    },
    // The network never answers: nothing drawn may wait for it.
    publish: (_h, p) => {
      published.push(p);
      const reply = new Promise<void>(() => {});
      void reply.then(() => {
        publishResolved = true;
      });
      return reply;
    },
    leave: async () => {},
  };
  return { bridge, published, emit: (e: PresenceEvent) => act(() => handler?.(e)), networkAnswered: () => publishResolved };
}

function cursor(x: number, y: number, extra: Partial<NonNullable<PresenceEvent["cursor"]>> = {}) {
  return { displayId: "", x, y, visible: true, pressed: false, shape: "arrow", shapeSource: "unspecified", atMs: 0, receivedMs: 0, ...extra };
}

/** Mounts a stream surface (800x500 at 100,50) with the overlay. */
async function mount(spaceId: string, opts: { interactive?: boolean; windowId?: string } = {}) {
  const fake = fakeBridge();
  let clock = 1_000_000;
  let layer: PresenceLayer | null = null;
  const utils = render(
    <div className="host" data-testid="host">
      <canvas data-testid="surface" />
      <PresenceOverlay
        spaceId={spaceId}
        interactive={opts.interactive ?? true}
        {...(opts.windowId ? { windowId: opts.windowId } : {})}
        bridge={fake.bridge}
        now={() => clock}
        onLayer={(l) => {
          layer = l;
        }}
      />
    </div>,
  );
  const surface = utils.getByTestId("surface") as HTMLCanvasElement;
  surface.getBoundingClientRect = () => ({ left: 100, top: 50, width: 800, height: 500, right: 900, bottom: 550, x: 100, y: 50, toJSON: () => ({}) });
  await waitFor(() => expect(layer).not.toBeNull());
  return {
    ...fake,
    ...utils,
    surface,
    layer: () => layer as unknown as PresenceLayer,
    host: utils.getByTestId("host"),
    setClock: (t: number) => {
      clock = t;
    },
    clock: () => clock,
    cursorOf: (id: string) => utils.container.querySelector(`.presence-cursor[data-participant="${id}"]`) as HTMLElement | null,
  };
}

describe("PresenceOverlay", () => {
  it("draws my cursor at the local pointer without waiting for the network, in my color", async () => {
    const m = await mount("s-own");
    pointer(m.surface, "pointermove", 300, 150);
    const own = m.container.querySelector(".presence-cursor-me") as HTMLElement;
    expect(own).not.toBeNull();
    expect(own.style.transform).toBe("translate(300px, 150px)");
    expect(own.innerHTML).toContain(`fill="${ME.color}"`);
    expect(own.dataset.shape).toBe("arrow");
    // Drawn before any reply: the publish never resolves in this test.
    expect(m.networkAnswered()).toBe(false);
    expect(m.published[0]).toEqual({ x: 0.25, y: 0.2, visible: true });
    // My own cursor has no name pill.
    expect(own.querySelector(".presence-cursor-name")).toBeNull();
  });

  it("hides the system cursor only over the stream", async () => {
    const m = await mount("s-hide");
    pointer(m.surface, "pointermove", 500, 300);
    expect(m.surface.style.cursor).toBe("none");
    expect(m.host.style.cursor).toBe("");
    pointer(m.surface, "pointerleave", 0, 0);
    expect(m.surface.style.cursor).toBe("");
    expect((m.container.querySelector(".presence-cursor-me") as HTMLElement).style.display).toBe("none");
    expect(m.published.at(-1)).toEqual({ x: 0, y: 0, visible: false });
  });

  it("view-only mirrors neither hide the pointer nor publish", async () => {
    const m = await mount("s-pip", { interactive: false });
    pointer(m.surface, "pointermove", 500, 300);
    expect(m.surface.style.cursor).toBe("");
    expect(m.published).toHaveLength(0);
  });

  it("gives my cursor the shape the server reports for me", async () => {
    const m = await mount("s-shape");
    pointer(m.surface, "pointermove", 300, 150);
    m.emit({ kind: "shape_changed", participantId: "me", shape: "text", shapeSource: "probe" });
    pointer(m.surface, "pointermove", 310, 150);
    expect((m.container.querySelector(".presence-cursor-me") as HTMLElement).dataset.shape).toBe("text");
  });

  it("draws others interpolated, shaped and named, and removes an agent when its run ends", async () => {
    const m = await mount("s-others");
    const t0 = m.clock();
    m.emit({ kind: "joined", participant: AGENT });
    m.emit({ kind: "cursor_moved", participantId: "agent-1", cursor: cursor(0.5, 0.5, { shape: "pointer", shapeSource: "hit_test", receivedMs: t0 }) });
    m.layer().render(t0 + 200);
    const el = m.cursorOf("agent-1") as HTMLElement;
    expect(el).not.toBeNull();
    expect(el.dataset.shape).toBe("pointer");
    expect(el.style.transform).toBe("translate(500px, 300px)");
    expect(el.querySelector(".presence-cursor-name")?.textContent).toBe("Claude");
    m.emit({ kind: "left", participantId: "agent-1", reason: "run_ended" });
    m.layer().render(t0 + 250);
    expect(m.cursorOf("agent-1")).toBeNull();
  });

  it("removes a participant a heartbeat does not list, and everyone when heartbeats stop", async () => {
    const m = await mount("s-beat");
    const t0 = m.clock();
    for (const p of [DANA, AGENT]) {
      m.emit({ kind: "joined", participant: p });
      m.emit({ kind: "cursor_moved", participantId: p.participantId, cursor: cursor(0.2, 0.2, { receivedMs: t0 }) });
    }
    m.layer().render(t0 + 200);
    expect(m.cursorOf("dana")).not.toBeNull();
    m.emit({ kind: "heartbeat", participantIds: ["me", "dana"] });
    m.layer().render(t0 + 300);
    expect(m.cursorOf("agent-1")).toBeNull();
    expect(m.cursorOf("dana")).not.toBeNull();
    // Three missed 5 s heartbeats: the rest go too (the view saw the last at t0).
    m.layer().render(t0 + 15_001);
    expect(m.cursorOf("dana")).toBeNull();
  });

  it("fades an idle cursor and then hides it", async () => {
    const m = await mount("s-idle");
    const t0 = m.clock();
    m.emit({ kind: "joined", participant: DANA });
    m.emit({ kind: "cursor_moved", participantId: "dana", cursor: cursor(0.3, 0.3, { receivedMs: t0 }) });
    m.layer().render(t0 + 1_000);
    expect(m.cursorOf("dana")?.style.opacity).toBe("1");
    m.layer().render(t0 + 5_150);
    expect(Number(m.cursorOf("dana")?.style.opacity)).toBeCloseTo(0.5, 2);
    m.layer().render(t0 + 5_400);
    expect(m.cursorOf("dana")).toBeNull();
    // It moves again: back at full opacity.
    m.emit({ kind: "cursor_moved", participantId: "dana", cursor: cursor(0.4, 0.3, { receivedMs: t0 + 5_500 }) });
    m.layer().render(t0 + 5_700);
    expect(m.cursorOf("dana")?.style.opacity).toBe("1");
  });

  it("only draws cursors on this surface's target", async () => {
    const m = await mount("s-target", { windowId: "w7" });
    const t0 = m.clock();
    m.emit({ kind: "joined", participant: DANA });
    m.emit({ kind: "joined", participant: AGENT });
    m.emit({ kind: "cursor_moved", participantId: "dana", cursor: cursor(0.1, 0.1, { windowId: "w7", receivedMs: t0 }) });
    m.emit({ kind: "cursor_moved", participantId: "agent-1", cursor: cursor(0.1, 0.1, { receivedMs: t0 }) });
    m.layer().render(t0 + 200);
    expect(m.cursorOf("dana")).not.toBeNull();
    expect(m.cursorOf("agent-1")).toBeNull();
    pointer(m.surface, "pointermove", 500, 300);
    expect(m.published.at(-1)).toEqual({ x: 0.5, y: 0.5, visible: true, windowId: "w7" });
  });
});

describe("shaped, colored cursors", () => {
  for (const color of ["#e6194b", "#4363d8"]) {
    it(`renders every shape in ${color}`, () => {
      const out: Record<string, string> = {};
      for (const shape of CURSOR_SHAPES) {
        const el = document.createElement("div");
        renderCursor(el, shape, color, shape === "arrow" ? "Dana" : null);
        out[shape] = el.innerHTML;
        expect(el.innerHTML).toContain(`fill="${color}"`);
      }
      expect(out).toMatchSnapshot();
    });
  }
});

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `presence.join`, `presence.publish`, `presence.leave`: one SDK presence
// session per page and Space, its events to the joining window only, in the
// web contract's shapes.
import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vitest";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import type { BridgeContext } from "../src/bridge/context";
import { BridgeEvents } from "../src/bridge/host";
import { presenceIdentity, presenceMethods, syntheticPresence, type PresenceSessionLike } from "../src/bridge/presence";
import type { PresenceCursor, PresenceEvent, PresenceIdentity } from "../src/native/generated/index";

const ADA = { participantId: "p1", principalId: "user:ada", displayName: "Ada", color: "#e6194b", kind: "human" };
const BOB = { participantId: "p2", principalId: "user:bob", displayName: "Bob", color: "#4363d8", kind: "human" };

/** A session whose events the test feeds. */
function fakeSession() {
  const queue: (PresenceEvent | undefined | Error)[] = [];
  const waiting: ((e: PresenceEvent | undefined | Error) => void)[] = [];
  const cursors: PresenceCursor[] = [];
  let left = 0;
  const session: PresenceSessionLike = {
    me: async () => ADA,
    roster: async () => [{ participant: ADA }, { participant: BOB }],
    nextEvent: () =>
      new Promise((resolve, reject) => {
        const take = (e: PresenceEvent | undefined | Error) => (e instanceof Error ? reject(e) : resolve(e));
        const next = queue.shift();
        if (next !== undefined || queue.length > 0) take(next);
        else waiting.push(take);
      }),
    updateCursor: async (c) => void cursors.push(c),
    usesDatagrams: () => true,
    leave: async () => void (left += 1),
  };
  const push = (e: PresenceEvent | undefined | Error) => {
    const w = waiting.shift();
    if (w) w(e);
    else queue.push(e);
  };
  return { session, push, cursors, left: () => left };
}

function context(signedIn = true) {
  const events = new BridgeEvents();
  const ctx = {
    model: {
      spaces: [{ id: "local:a" }],
      identity: signedIn ? "ada@example.com" : null,
      account: { profile: () => ({ name: "Ada Lovelace", email: "ada@example.com", subject: "sub-1" }) },
      native: {
        appPresenceName: (name?: string, _e?: string, _u?: string, _f?: string, os?: string) => name ?? os ?? "",
        appPresencePrincipalId: (email?: string, _s?: string, _u?: string, os?: string) => (email ? `user:${email}` : `os:${os}`),
      },
    },
    events,
    env: {},
  } as unknown as BridgeContext;
  return { ctx, events };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("presence", () => {
  it("joins as the signed-in account, else this computer's user", () => {
    const { ctx } = context();
    expect(presenceIdentity(ctx.model, "ada")).toEqual({ id: "user:ada@example.com", displayName: "Ada Lovelace", color: "", agent: false });
    const out = context(false);
    expect(presenceIdentity(out.ctx.model, "ada")).toEqual({ id: "os:ada", displayName: "ada", color: "", agent: false });
  });

  it("answers the join in the contract's shape and sends the session's events to the joining window only", async () => {
    const { ctx, events } = context();
    const fake = fakeSession();
    const joins: { spaceId: string; identity: PresenceIdentity }[] = [];
    const m = presenceMethods(ctx, async (spaceId, identity) => {
      joins.push({ spaceId, identity });
      return fake.session;
    });
    const sent: { event: string; payload: unknown; to: unknown }[] = [];
    events.subscribe((e, to) => sent.push({ event: e.event, payload: e.payload, to }));
    const win = Object.assign(new EventEmitter(), { webContents: new EventEmitter() });
    const info = await m["presence.join"]!({ spaceId: "local:a" }, { window: win });
    expect(validate(OP_SHAPES["presence.join"], info, SHAPE_DEFS)).toEqual([]);
    expect(info).toMatchObject({ handle: "p1", me: ADA, delayMs: 66, datagrams: true });
    expect(joins.map((j) => j.spaceId)).toEqual(["local:a"]);

    const moved: PresenceEvent = { kind: "cursor_moved", participantId: "p2", cursor: { displayId: "", x: 0.1, y: 0.2, visible: true, pressed: false, shape: "arrow", shapeSource: "probe", atMs: 1, receivedMs: 2 } };
    fake.push(moved);
    await tick();
    expect(sent).toEqual([{ event: "presence.event", payload: { handle: "p1", event: moved }, to: win }]);

    // A timeout is a quiet second, not the end.
    fake.push(Object.assign(new Error("timed out"), { tag: "Timeout" }));
    await tick();
    expect(fake.left()).toBe(0);

    expect(await m["presence.publish"]!({ handle: "p1", x: 0.5, y: 0.25, visible: true, windowId: "w7" }, {})).toBeNull();
    expect(fake.cursors).toEqual([expect.objectContaining({ x: 0.5, y: 0.25, visible: true, windowId: "w7", displayId: "" })]);
    await expect(m["presence.publish"]!({ handle: "p1", x: "left", y: 0, visible: true }, {})).rejects.toMatchObject({ code: "bad_args" });

    // Closing the window leaves.
    win.emit("closed");
    fake.push({ kind: "keep_alive" });
    await tick();
    await tick();
    expect(fake.left()).toBe(1);
    expect(sent).toHaveLength(1);
    await expect(m["presence.publish"]!({ handle: "p1", x: 0, y: 0, visible: false }, {})).rejects.toMatchObject({ code: "not_found" });
  });

  it("leaves when the window loads a new page", async () => {
    const { ctx } = context();
    const fake = fakeSession();
    const m = presenceMethods(ctx, async () => fake.session);
    const win = Object.assign(new EventEmitter(), { webContents: new EventEmitter() });
    await m["presence.join"]!({ spaceId: "local:a" }, { window: win });
    win.webContents.emit("did-navigate");
    fake.push({ kind: "keep_alive" });
    await tick();
    await tick();
    expect(fake.left()).toBe(1);
  });

  it("leaves when asked, and when the stream ends", async () => {
    const { ctx } = context();
    const a = fakeSession();
    const b = fakeSession();
    const sessions = [a.session, b.session];
    const m = presenceMethods(ctx, async () => sessions.shift()!);
    await m["presence.join"]!({ spaceId: "local:a" }, {});
    expect(m["presence.leave"]!({ handle: "p1" }, {})).toBeNull();
    a.push({ kind: "keep_alive" });
    await tick();
    await tick();
    expect(a.left()).toBe(1);

    await m["presence.join"]!({ spaceId: "local:a" }, {});
    b.push(undefined);
    await tick();
    await tick();
    expect(b.left()).toBe(1);
  });

  it("refuses a Space this app doesn't list", async () => {
    const { ctx } = context();
    const join = vi.fn();
    await expect(presenceMethods(ctx, join)["presence.join"]!({ spaceId: "local:gone" }, {})).rejects.toMatchObject({ code: "not_found" });
    expect(join).not.toHaveBeenCalled();
  });

  it("plays two participants circling the picture for screenshots", async () => {
    const s = syntheticPresence(() => 1_000);
    const roster = await s.roster();
    expect(roster.map((m) => m.participant.displayName)).toEqual(["You", "Ada Lovelace", "Claude"]);
    const e = await s.nextEvent(undefined);
    expect(e).toMatchObject({ kind: "cursor_moved", participantId: "ada" });
    expect(e?.cursor?.x).toBeGreaterThan(0);
    expect(e?.cursor?.x).toBeLessThan(1);
    await s.leave();
    expect(await s.nextEvent(undefined)).toBeUndefined();
  });
});

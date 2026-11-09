// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Presence over the video the page draws (`presence.join`, `presence.publish`,
// `presence.leave`; the web bridge's ops/presence.ts): one `PresenceService`
// session per page and Space, joined through the cua SDK
// (`Space.joinPresence`), as the SwiftUI app's `LiveStreamSession` and the
// Tauri app's `presence_join` do. This only moves data; the page folds the
// events into the SDK's `PresenceView` and draws.
//
// - `presence.join` joins as the signed-in account (else this computer's
//   user), with the app core's name and principal id, and answers the
//   caller, the roster and the render delay for the transport;
// - every event goes to the joining window only, as `presence.event`
//   `{handle, event}`, in the TS SDK's `PresenceEvent` shape (the binding's
//   records already are);
// - `presence.publish` moves your cursor (the SDK throttles to 30 Hz, newest
//   wins, show and hide at once);
// - `presence.leave`, the window closing or loading a new page (a reload),
//   or the stream ending leaves.
//
// `CUA_SPACES_TEST_PRESENCE=1` joins every Space as a synthetic session with
// two participants circling the picture, for screenshots and the WebCodecs
// harness (`CUA_SPACES_TEST_STREAM_URL`).
import { userInfo } from "node:os";
import type { PresenceCursor, PresenceEvent, PresenceIdentity, PresenceMember, PresenceParticipant } from "../native/generated/index";
import type { AppModel } from "../model/app-model";
import { string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";

/** How long a join may take before the page goes without presence. */
export const JOIN_TIMEOUT_MS = 10_000;
/** One `nextEvent` wait; the pump checks for a leave between waits. */
export const EVENT_WAIT_MS = 1_000;
/** Remote cursors are drawn this far in the past, per transport (the Tauri app's). */
export const STREAM_DELAY_MS = 100;
export const DATAGRAM_DELAY_MS = 66;
/** Consecutive failed waits before the session is given up (the SwiftUI app's `presenceRetries`). */
export const MAX_FAILURES = 5;

/** The parts of the SDK's `SpacePresence` this uses. */
export interface PresenceSessionLike {
  me(): Promise<PresenceParticipant>;
  roster(): Promise<PresenceMember[]>;
  nextEvent(timeoutMs: bigint | undefined): Promise<PresenceEvent | undefined>;
  updateCursor(cursor: PresenceCursor): Promise<void>;
  usesDatagrams(): boolean;
  leave(): Promise<void>;
}

/** Joins a Space's presence as `identity`. */
export type PresenceJoiner = (spaceId: string, identity: PresenceIdentity) => Promise<PresenceSessionLike>;

/** Who this app joins as (the Tauri app's `identity`, the SwiftUI app's `presenceName`): the core's name and id for the signed-in account, else this computer's user. */
export function presenceIdentity(model: Pick<AppModel, "native" | "identity" | "account">, osUser: string | undefined): PresenceIdentity {
  const p = model.identity ? model.account?.profile() : null;
  return {
    id: model.native.appPresencePrincipalId(p?.email, p?.subject, p?.username, osUser),
    displayName: model.native.appPresenceName(p?.name, p?.email, p?.username, undefined, osUser),
    color: "",
    agent: false,
  };
}

const isTimeout = (error: unknown) => (error as { tag?: unknown } | null)?.tag === "Timeout";

interface Joined {
  session: PresenceSessionLike;
  window: unknown;
  left: boolean;
}

/** The live sessions by handle, and the pumps that forward their events. */
export class PresenceSessions {
  private next = 0;
  private readonly live = new Map<string, Joined>();

  constructor(
    private readonly join: PresenceJoiner,
    /** Sends one event to one window. */
    private readonly send: (window: unknown, payload: { handle: string; event: PresenceEvent }) => void,
  ) {}

  async open(spaceId: string, identity: PresenceIdentity, window: unknown) {
    const session = await this.join(spaceId, identity);
    const [me, members] = await Promise.all([session.me(), session.roster()]);
    const handle = `p${++this.next}`;
    const joined: Joined = { session, window, left: false };
    this.live.set(handle, joined);
    onGone(window, () => this.leave(handle));
    void this.pump(handle, joined);
    const datagrams = session.usesDatagrams();
    return { handle, me, members, delayMs: datagrams ? DATAGRAM_DELAY_MS : STREAM_DELAY_MS, datagrams };
  }

  async publish(handle: string, cursor: Pick<PresenceCursor, "x" | "y" | "visible" | "windowId">): Promise<void> {
    const joined = this.live.get(handle);
    if (!joined) throw Failure.notFound(`no presence session ${handle}`);
    await joined.session.updateCursor({
      displayId: "",
      ...(cursor.windowId ? { windowId: cursor.windowId } : {}),
      x: cursor.x,
      y: cursor.y,
      visible: cursor.visible,
      pressed: false,
      shape: "arrow",
      shapeSource: "unspecified",
      atMs: 0,
      receivedMs: 0,
    });
  }

  leave(handle: string): void {
    const joined = this.live.get(handle);
    if (!joined) return;
    this.live.delete(handle);
    joined.left = true;
  }

  /** Live handles (tests). */
  get handles(): string[] {
    return [...this.live.keys()];
  }

  /** Forwards events until a leave, the stream's end or repeated failures; then leaves the Space. */
  private async pump(handle: string, joined: Joined): Promise<void> {
    let failures = 0;
    while (!joined.left) {
      let event: PresenceEvent | undefined;
      try {
        event = await joined.session.nextEvent(BigInt(EVENT_WAIT_MS));
        failures = 0;
      } catch (error) {
        if (isTimeout(error)) continue;
        if (++failures >= MAX_FAILURES) break;
        continue;
      }
      if (!event) break;
      if (!joined.left) this.send(joined.window, { handle, event });
    }
    this.leave(handle);
    await joined.session.leave().catch(() => {});
  }
}

type Once = { once?: (event: string, f: () => void) => void };

/** Calls `f` when an Electron window closes or loads a new page, whose own sessions are gone (no Electron here: duck-typed). */
function onGone(window: unknown, f: () => void): void {
  let done = false;
  const once = () => {
    if (done) return;
    done = true;
    f();
  };
  const w = window as (Once & { webContents?: Once }) | null;
  w?.once?.("closed", once);
  w?.webContents?.once?.("did-navigate", once);
}

const finite = (args: BridgeArgs, key: string): number => {
  const v = args[key];
  if (typeof v !== "number" || !Number.isFinite(v)) throw Failure.badArgs(`${key}: number`);
  return v;
};

export function presenceMethods(ctx: BridgeContext, joiner?: PresenceJoiner): Handlers {
  const { model, events, env } = ctx;
  const join: PresenceJoiner =
    joiner ??
    (env.CUA_SPACES_TEST_PRESENCE === "1"
      ? async () => syntheticPresence()
      : async (spaceId, identity) => model.backend.joinPresence(spaceId, identity, JOIN_TIMEOUT_MS));
  const sessions = new PresenceSessions(join, (window, payload) => events.emit("presence.event", payload, window));
  let osUser: string | undefined;
  try {
    osUser = userInfo().username || undefined;
  } catch {
    osUser = undefined;
  }
  return {
    "presence.join": async (args, caller) => {
      const spaceId = string(args, "spaceId");
      if (!model.spaces.some((s) => s.id === spaceId) && env.CUA_SPACES_TEST_PRESENCE !== "1") throw Failure.notFound(`no Space ${spaceId}`);
      return sessions.open(spaceId, presenceIdentity(model, osUser), caller.window);
    },
    "presence.publish": async (args) => {
      const windowId = args.windowId;
      if (windowId !== undefined && typeof windowId !== "string") throw Failure.badArgs("windowId: string");
      if (typeof args.visible !== "boolean") throw Failure.badArgs("visible: boolean");
      await sessions.publish(string(args, "handle"), { x: finite(args, "x"), y: finite(args, "y"), visible: args.visible, windowId: windowId || undefined });
      return null;
    },
    "presence.leave": (args) => {
      sessions.leave(string(args, "handle"));
      return null;
    },
  };
}

/**
 * A session with two participants circling the picture (Ada slowly, an
 * agent faster, the agent's cursor a hand), one move each per ~33 ms. Your
 * own cursor is accepted and dropped.
 */
export function syntheticPresence(now: () => number = Date.now): PresenceSessionLike {
  const me: PresenceParticipant = { participantId: "you", principalId: "user:you", displayName: "You", color: "#4363d8", kind: "human" };
  const others: { p: PresenceParticipant; period: number; radius: number; shape: string }[] = [
    { p: { participantId: "ada", principalId: "user:ada", displayName: "Ada Lovelace", color: "#e6194b", kind: "human" }, period: 9_000, radius: 0.28, shape: "arrow" },
    { p: { participantId: "agent", principalId: "agent:claude", displayName: "Claude", color: "#f58231", kind: "agent" }, period: 5_000, radius: 0.18, shape: "pointer" },
  ];
  let turn = 0;
  let left = false;
  const at = (o: (typeof others)[number], t: number): PresenceCursor => {
    const a = (2 * Math.PI * (t % o.period)) / o.period;
    return { displayId: "", x: 0.5 + o.radius * Math.cos(a), y: 0.5 + o.radius * Math.sin(a), visible: true, pressed: false, shape: o.shape, shapeSource: "probe", atMs: t, receivedMs: t };
  };
  return {
    me: async () => me,
    roster: async () => [{ participant: me }, ...others.map((o) => ({ participant: o.p, cursor: at(o, now()) }))],
    nextEvent: async () => {
      await new Promise((r) => setTimeout(r, 33 / others.length));
      if (left) return undefined;
      const o = others[turn++ % others.length]!;
      return { kind: "cursor_moved", participantId: o.p.participantId, cursor: at(o, now()) };
    },
    updateCursor: async () => {},
    usesDatagrams: () => false,
    leave: async () => {
      left = true;
    },
  };
}

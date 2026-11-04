// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Presence for the web viewer over gRPC-Web: joins `PresenceService` (asking
 * for cursor shapes, 5 s roster heartbeats and cursor batches), folds the
 * stream into the SDK's `PresenceView`, and draws it with the shared
 * `PresenceLayer`. Browsers have no QUIC datagrams here, so cursors travel on
 * the Join stream and `UpdateCursor`.
 */

import { timestampMs } from "@bufbuild/protobuf/wkt";
import { HEARTBEAT_INTERVAL_MS, PresenceView, type PresenceEvent, type PresenceParticipant } from "@trycua/cua/spaces/presence";

import type { Api } from "./api";
import { PresenceLayer } from "./core/presenceLayer";
import {
  CursorShape,
  CursorShapeSource,
  LeaveReason,
  type CursorMoved,
  type CursorPosition,
  type JoinResponse,
  type Participant,
} from "./gen/cua/env/v1/presence_pb";
import { PrincipalKind } from "./gen/cua/env/v1/common_pb";

const SHAPES: Record<number, string> = {
  [CursorShape.UNSPECIFIED]: "arrow",
  [CursorShape.ARROW]: "arrow",
  [CursorShape.TEXT]: "text",
  [CursorShape.POINTER]: "pointer",
  [CursorShape.RESIZE_NS]: "resize_ns",
  [CursorShape.RESIZE_EW]: "resize_ew",
  [CursorShape.RESIZE_NESW]: "resize_nesw",
  [CursorShape.RESIZE_NWSE]: "resize_nwse",
  [CursorShape.WAIT]: "wait",
  [CursorShape.PROGRESS]: "progress",
  [CursorShape.NOT_ALLOWED]: "not_allowed",
  [CursorShape.CROSSHAIR]: "crosshair",
  [CursorShape.GRAB]: "grab",
  [CursorShape.GRABBING]: "grabbing",
  [CursorShape.MOVE]: "move",
};
const SOURCES: Record<number, string> = {
  [CursorShapeSource.UNSPECIFIED]: "unspecified",
  [CursorShapeSource.HIT_TEST]: "hit_test",
  [CursorShapeSource.SYSTEM]: "system",
  [CursorShapeSource.PROBE]: "probe",
};
const REASONS: Record<number, string> = {
  [LeaveReason.UNSPECIFIED]: "",
  [LeaveReason.LEFT]: "left",
  [LeaveReason.DISCONNECTED]: "disconnected",
  [LeaveReason.TIMEOUT]: "timeout",
  [LeaveReason.RUN_ENDED]: "run_ended",
};

export function participantOf(p: Participant | undefined): PresenceParticipant {
  const principal = p?.principal;
  return {
    participantId: p?.participantId ?? "",
    principalId: principal?.id ?? "",
    displayName: principal?.displayName ?? "",
    color: principal?.color ?? "",
    kind: principal?.kind === PrincipalKind.AGENT ? "agent" : principal?.kind === PrincipalKind.HUMAN ? "human" : "unspecified",
  };
}

function cursorOf(c: CursorPosition | undefined, atMs: number, receivedMs: number): NonNullable<PresenceEvent["cursor"]> {
  return {
    displayId: c?.displayId ?? "",
    ...(c?.window?.id ? { windowId: c.window.id } : {}),
    x: c?.position?.x ?? 0,
    y: c?.position?.y ?? 0,
    visible: c?.visible ?? false,
    pressed: false,
    shape: SHAPES[c?.shape ?? 0] ?? "arrow",
    shapeSource: SOURCES[c?.shapeSource ?? 0] ?? "unspecified",
    atMs,
    receivedMs,
  };
}

function moved(m: CursorMoved, receivedMs: number): PresenceEvent {
  return {
    kind: "cursor_moved",
    participantId: m.participantId,
    cursor: cursorOf(m.cursor, m.at ? timestampMs(m.at) : 0, receivedMs),
  };
}

/** One Join stream message as the SDK's presence events (batches flattened). */
export function presenceEvents(message: JoinResponse, receivedMs: number): PresenceEvent[] {
  const e = message.event;
  switch (e.case) {
    case "joined":
      return [{ kind: "joined", participant: participantOf(e.value.participant) }];
    case "participantJoined":
      return [{ kind: "joined", participant: participantOf(e.value) }];
    case "participantLeft":
      return [{ kind: "left", participantId: e.value.participantId, reason: REASONS[e.value.reason] ?? "" }];
    case "cursorMoved":
      return [moved(e.value, receivedMs)];
    case "cursorBatch":
      return e.value.moves.map((m) => moved(m, receivedMs));
    case "cursorShapeChanged":
      return [
        {
          kind: "shape_changed",
          participantId: e.value.participantId,
          shape: SHAPES[e.value.shape] ?? "arrow",
          shapeSource: SOURCES[e.value.source] ?? "unspecified",
        },
      ];
    case "rosterHeartbeat":
      return [{ kind: "heartbeat", participantIds: [...e.value.participantIds] }];
    case "keepalive":
      return [{ kind: "keep_alive" }];
    default:
      return [];
  }
}

/** A joined viewer: the model plus its drawing layer. */
export class ViewerPresence {
  private readonly abort = new AbortController();
  private view: PresenceView | null = null;
  private me: PresenceParticipant | null = null;
  private layer: PresenceLayer | null = null;
  private stopped = false;

  constructor(
    private readonly api: Api,
    private readonly surface: HTMLCanvasElement,
    private readonly host: HTMLElement,
    private readonly interactive: boolean,
    private readonly displayId: string,
    private readonly now: () => number = Date.now,
    /** Whether the stream is live: a lost stream draws no cursor of yours. */
    private readonly live: () => boolean = () => true,
  ) {}

  /** Joins and pumps events until `stop`; resolves false when the Space has no presence. */
  async start(): Promise<boolean> {
    const stream = this.api.presence.join(
      { cursorShapes: true, cursorBatches: true, rosterInterval: { seconds: BigInt(HEARTBEAT_INTERVAL_MS / 1000), nanos: 0 } },
      { signal: this.abort.signal },
    );
    try {
      for await (const message of stream) {
        if (this.stopped) break;
        if (!this.view) {
          if (message.event.case !== "joined") continue;
          this.onJoined(message);
          continue;
        }
        const t = this.now();
        for (const e of presenceEvents(message, t)) this.view.apply(e, t);
      }
    } catch {
      // Aborted, or the Space has no presence: the viewer streams without it.
    }
    return this.view !== null;
  }

  private onJoined(message: JoinResponse): void {
    if (message.event.case !== "joined") return;
    const joined = message.event.value;
    const me = participantOf(joined.participant);
    const t = this.now();
    this.me = me;
    this.view = PresenceView.from(
      me,
      joined.roster.map((r) => ({
        participant: participantOf(r.participant),
        ...(r.cursor ? { cursor: cursorOf(r.cursor, 0, t) } : {}),
      })),
      undefined,
      t,
    );
    this.view.setHeartbeatIntervalMs(HEARTBEAT_INTERVAL_MS);
    this.layer = new PresenceLayer({
      surface: this.surface,
      host: this.host,
      view: () => this.view,
      me: () => (this.me ? { id: this.me.participantId, color: this.view?.participant(this.me.participantId)?.color || this.me.color } : null),
      interactive: this.interactive,
      live: this.live,
      onPointer: (p) => {
        if (!this.me) return;
        void this.api.presence
          .updateCursor({
            participantId: this.me.participantId,
            cursor: {
              displayId: this.displayId === "primary" ? "" : this.displayId,
              position: { x: p.x, y: p.y },
              visible: p.visible,
            },
          })
          .catch(() => {});
      },
      now: this.now,
    });
  }

  stop(): void {
    if (this.stopped) return;
    this.stopped = true;
    this.layer?.destroy();
    this.layer = null;
    const me = this.me;
    this.abort.abort();
    if (me) void this.api.presence.leave({ participantId: me.participantId }).catch(() => {});
  }
}

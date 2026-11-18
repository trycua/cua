// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Presence over a live stream the page draws (WebCodecs): the other
 * participants' cursors, and yours published to them. One `PresenceService`
 * session per Space per window, joined through the cua SDK in the host
 * (`Space.joinPresence`, as the SwiftUI app's `LiveStreamSession` and the
 * Tauri app's `presence_join`). The page folds the events into the SDK's
 * `PresenceView` and draws with the shared `PresenceLayer`
 * (`components/video/presence.ts`).
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `presence.join {spaceId}` | the host method `presence.join` (`src/bridge/presence.ts`) | unsupported: its native viewer draws presence (`PresenceOverlay`) | unsupported: its viewer windows join with `presence_join` |
 * | `presence.publish {handle, x, y, visible, windowId?}` | `presence.publish` | unsupported | unsupported |
 * | `presence.leave {handle}` | `presence.leave` | unsupported | unsupported |
 *
 * Events arrive on the joining window only, as `presence.event`
 * `{handle, event}` (`event` in the TS SDK's `PresenceEvent` shape).
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";

/** A participant (the TS SDK's `PresenceParticipant`). */
export interface PresenceParticipantWire {
  participantId: string;
  principalId: string;
  displayName: string;
  color: string;
  kind: string;
}

/** A cursor (the TS SDK's `PresenceCursor`). */
export interface PresenceCursorWire {
  displayId: string;
  windowId?: string;
  x: number;
  y: number;
  visible: boolean;
  pressed: boolean;
  shape: string;
  shapeSource: string;
  atMs: number;
  receivedMs: number;
}

/** What `presence.join` answers. */
export interface PresenceJoinInfo {
  /** Names the session in `publish`, `leave` and its events. */
  handle: string;
  me: PresenceParticipantWire;
  members: { participant: PresenceParticipantWire; cursor?: PresenceCursorWire }[];
  /** Render delay for remote cursors on this transport (ms). */
  delayMs: number;
  datagrams: boolean;
}

/** The local pointer, normalized to the streamed surface. */
export interface PresencePointerArgs {
  handle: string;
  x: number;
  y: number;
  visible: boolean;
  windowId?: string;
}

/** The pushed event's name and payload. */
export const PRESENCE_EVENT = "presence.event";

export interface PresenceOps {
  "presence.join": { args: { spaceId: string }; result: PresenceJoinInfo };
  "presence.publish": { args: PresencePointerArgs; result: null };
  "presence.leave": { args: { handle: string }; result: null };
}

export const PRESENCE_OPERATIONS = ["presence.join", "presence.publish", "presence.leave"] as const satisfies readonly (keyof PresenceOps)[];

const UNSUPPORTED = {
  webkit: "the Mac app draws presence natively over its own video (PresenceOverlay)",
  tauri: "the Tauri app's viewer windows join presence themselves (presence_join)",
};

export const PRESENCE_COVERAGE = {
  "presence.join": { webkit: { methods: [] }, electron: { methods: ["presence.join"] }, tauri: [], unsupported: UNSUPPORTED },
  "presence.publish": { webkit: { methods: [] }, electron: { methods: ["presence.publish"] }, tauri: [], unsupported: UNSUPPORTED },
  "presence.leave": { webkit: { methods: [] }, electron: { methods: ["presence.leave"] }, tauri: [], unsupported: UNSUPPORTED },
} as const satisfies Record<keyof PresenceOps, OpCoverage>;

/** Hosts where the page draws no presence. */
export function unsupportedPresenceOps(mode: BridgeMode) {
  const no = (op: keyof PresenceOps) => () => Promise.reject(new UnsupportedOperationError(mode, op));
  return {
    "presence.join": no("presence.join") as () => Promise<PresenceJoinInfo>,
    "presence.publish": no("presence.publish") as () => Promise<null>,
    "presence.leave": no("presence.leave") as () => Promise<null>,
  };
}

/** Arguments that take each down its usual path (coverage tests). */
export const PRESENCE_ARGS = {
  "presence.join": { spaceId: "local:design-review" },
  "presence.publish": { handle: "p1", x: 0.5, y: 0.5, visible: true },
  "presence.leave": { handle: "p1" },
};

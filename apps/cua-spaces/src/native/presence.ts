// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { PresenceEvent, PresenceMember, PresenceParticipant } from "@trycua/cua/spaces/presence";

import { hasTauri } from "./bridge";

/**
 * The shell's presence commands (`src-tauri/src/presence.rs`): one
 * `PresenceService` session per viewer window, joined through the cua SDK.
 * Outside Tauri there is no bridge and viewers draw no presence.
 */
export type { PresenceEvent, PresenceMember, PresenceParticipant };

export interface PresenceJoinInfo {
  handle: string;
  me: PresenceParticipant;
  members: PresenceMember[];
  /** Render delay for remote cursors on this transport (ms). */
  delayMs: number;
  datagrams: boolean;
}

/** Where the local pointer is, normalized to the streamed surface. */
export interface PresencePointer {
  x: number;
  y: number;
  visible: boolean;
  windowId?: string;
  displayId?: string;
}

export interface PresenceBridge {
  join(spaceId: string): Promise<PresenceJoinInfo>;
  /** Subscribes to one session's events; returns the unsubscribe. */
  onEvent(handle: string, handler: (event: PresenceEvent) => void): Promise<() => void>;
  publish(handle: string, pointer: PresencePointer): Promise<void>;
  leave(handle: string): Promise<void>;
}

interface Envelope {
  handle: string;
  event: PresenceEvent;
}

/** The Tauri bridge, or null outside the shell. */
export function createPresenceBridge(): PresenceBridge | null {
  if (!hasTauri()) return null;
  const core = import("@tauri-apps/api/core");
  const event = import("@tauri-apps/api/event");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) => (await core).invoke<T>(command, args);
  return {
    join: (spaceId) => invoke<PresenceJoinInfo>("presence_join", { spaceId }),
    onEvent: async (handle, handler) => {
      const { listen } = await event;
      return listen<Envelope>("presence-event", (e) => {
        if (e.payload.handle === handle) handler(e.payload.event);
      });
    },
    publish: (handle, p) =>
      invoke<void>("presence_publish", {
        handle,
        x: p.x,
        y: p.y,
        visible: p.visible,
        windowId: p.windowId ?? null,
        displayId: p.displayId ?? null,
      }),
    leave: (handle) => invoke<void>("presence_leave", { handle }),
  };
}

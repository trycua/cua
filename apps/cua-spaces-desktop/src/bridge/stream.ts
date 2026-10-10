// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's live desktop (or one window) for the page's WebCodecs player
// (`spaces.openStream {spaceId, tier, windowId?, epoch?, activate?}`, only this host: the Mac app draws
// video natively): a media ticket from this app's daemon
// (`media-bridge.ts`), started when it isn't running. The video goes from
// the page straight to the daemon's loopback listener, never through IPC.
import { cuaHome } from "../model/daemon";
import { createStreamOpener, StreamUnavailableError, type Fetch } from "../media-bridge";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

export function streamMethods({ supervisor, env }: BridgeContext): Handlers {
  const open = createStreamOpener({
    cuaHome: cuaHome(env),
    fetch: globalThis.fetch as unknown as Fetch,
    hasCua: async () => supervisor !== null,
    // Success is daemon.json appearing; the opener waits for it.
    startDaemon: async () => {
      await supervisor?.start();
    },
    testStreamUrl: env.CUA_SPACES_TEST_STREAM_URL || null,
  });
  return {
    "spaces.openStream": async (args) => {
      const { spaceId, tier, windowId, epoch, activate } = args;
      if (typeof spaceId !== "string" || spaceId === "") throw Failure.badArgs("spaceId: string");
      if (tier !== "tile" && tier !== "full") throw Failure.badArgs("tier: tile | full");
      if (windowId !== undefined && (typeof windowId !== "string" || windowId === "")) throw Failure.badArgs("windowId: string");
      if (epoch !== undefined && (typeof epoch !== "number" || !Number.isInteger(epoch) || epoch < 0)) throw Failure.badArgs("epoch: number");
      try {
        return await open({ spaceId, tier, windowId, epoch, activate: activate === true });
      } catch (error) {
        if (error instanceof StreamUnavailableError) throw Failure.unsupported(error.message);
        throw error;
      }
    },
  };
}

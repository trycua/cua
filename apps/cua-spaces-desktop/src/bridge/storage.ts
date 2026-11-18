// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → Storage (`storage.*`): the Cua Volume's store, mount and cache
// (the SwiftUI host's `StorageModel` and `storageRun`): `storage.get`
// answers the daemon's three tools, `storage.run` runs the command the
// core's section asked for, then the section reads again.
import { homeExpanded } from "../model/storage";
import { object, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";
import { daemonTool, hostParts } from "./host-parts";
import { homePath, settingsUrl } from "./volume";

export function storageMethods(ctx: BridgeContext): Handlers {
  const parts = () => hostParts(ctx);
  const run = async (request: BridgeArgs): Promise<unknown> => {
    const tool = daemonTool(ctx);
    switch (typeof request.kind === "string" ? request.kind : "") {
      case "test":
      case "save":
      case "adopt":
        return tool("volume_storage_set", object(request, "update"));
      case "mount":
        await tool("volume_mount");
        return null;
      case "unmount":
        await tool("volume_unmount");
        return null;
      case "reveal": {
        const system = parts().system;
        if (!system) throw Failure.unsupported("Showing files is not available in this build");
        system.reveal(homeExpanded(homePath(ctx, string(request, "path")), parts().storage.answers.home));
        return null;
      }
      case "open-url": {
        const system = parts().system;
        if (!system) throw Failure.unsupported("Opening settings is not available in this build");
        await system.openExternal(settingsUrl(string(request, "url"), ctx.platform));
        return null;
      }
      case "set-cache": {
        const bytes = request.capacity_bytes;
        if (typeof bytes !== "number") throw Failure.badArgs("capacity_bytes: number");
        await tool("volume_cache_set", { capacity_bytes: bytes });
        return null;
      }
      case "clear-cache":
        await tool("volume_cache_clear");
        return null;
      default:
        throw Failure.badArgs("request.kind");
    }
  };
  return {
    "storage.get": async () => {
      const storage = parts().storage;
      await storage.load();
      return storage.answers;
    },
    "storage.run": async (args) => {
      const request = object(args, "request");
      try {
        return await run(request);
      } finally {
        // The section reads again (in the background, as the Swift app does).
        void parts().storage.load();
      }
    },
  };
}

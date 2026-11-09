// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `app.info`: which host this is, its version, the experiments and the
// methods it routes (the SwiftUI host's `appInfo`).
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";
import { encode } from "./value";

/** The page's word for the system (`app.info.platform`). */
export function platformWord(platform: NodeJS.Platform): string {
  if (platform === "darwin") return "macos";
  if (platform === "win32") return "windows";
  return platform === "linux" ? "linux" : platform;
}

export function appMethods(ctx: BridgeContext): Handlers {
  return {
    "app.info": () => ({
      platform: platformWord(ctx.platform),
      host: "electron",
      version: ctx.version,
      experiments: encode(ctx.model.settings.experiments),
      methods: ctx.methods(),
    }),
  };
}

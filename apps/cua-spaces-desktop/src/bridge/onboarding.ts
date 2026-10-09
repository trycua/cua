// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The first run on this host (`onboarding.get`, `onboarding.complete`, only
// the Electron shell's: the SwiftUI app draws its own first run natively).
// The page runs the app core's flow (`routes/onboarding`) and asks here
// whether it is due; finishing saves `onboarding.json` as the Swift app's
// OnboardingModel.finish does, asks for Local Network access while someone
// is here (macOS), and applies Done's "Launch at login" checkbox.
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { hostParts } from "./host-parts";

export function onboardingMethods(ctx: BridgeContext): Handlers {
  const parts = () => hostParts(ctx);
  return {
    "onboarding.get": () => parts().onboarding.read(),
    "onboarding.complete": (args) => {
      const mode = args.mode;
      if (mode !== "client" && mode !== "host") throw Failure.badArgs("mode: client | host");
      parts().onboarding.finish(mode);
      // Every machine, host or not: its own macOS Spaces live on the local
      // network (vmnet), so ask while someone is here.
      ctx.model.host.requestLocalNetwork();
      if (typeof args.launchAtLogin === "boolean") parts().loginItem.set(args.launchAtLogin);
      ctx.model.changed("settings");
      return parts().onboarding.read();
    },
  };
}

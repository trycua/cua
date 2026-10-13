// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → About and launch at login (`about.*`, `loginItem.*`): the
// updater's controls, channel and checks (the SwiftUI host's
// `UpdatesModel`; here electron-updater, `src/updater.ts`), and the login
// item (`LoginItem`; here Electron's login item settings on macOS and
// Windows, an autostart entry on Linux, `src/login-item.ts`).
import { bool } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { hostParts } from "./host-parts";
import { encode } from "./value";

export function aboutMethods(ctx: BridgeContext): Handlers {
  const parts = () => hostParts(ctx);
  const updates = () => parts().updates;
  const withUpdater = () => {
    const u = updates();
    if (!u.updater) throw Failure.unsupported("Updates are off in this build");
    return u;
  };
  const loginItem = async () => {
    const report = await parts().loginItem.report();
    if (!report) throw Failure.unsupported("Launch at login is not available in this build");
    return report;
  };
  return {
    "about.get": () => encode(updates().input),
    "about.set": (args) => {
      const u = withUpdater();
      if (typeof args.autoCheck === "boolean") u.setAutoCheck(args.autoCheck);
      if (typeof args.autoInstall === "boolean") u.setAutoInstall(args.autoInstall);
      if (typeof args.channel === "string") u.choose(args.channel);
      return encode(u.input);
    },
    "about.checkNow": () => {
      const u = withUpdater();
      // The result is the updater's own window: start the check after this
      // answer, so the page never waits on someone closing it.
      setTimeout(() => u.checkNow(), 0);
      return { ...(encode(u.input) as Record<string, unknown>), checking: true };
    },
    "loginItem.get": () => loginItem(),
    "loginItem.set": async (args) => {
      const on = bool(args, "on");
      const model = parts().loginItem;
      if (!model.item) throw Failure.unsupported("Launch at login is not available in this build");
      model.set(on);
      if (model.error) throw Failure.failed(model.error);
      ctx.model.changed("settings");
      return loginItem();
    },
    "loginItem.openSettings": () => {
      parts().loginItem.item?.openSystemSettings();
      return null;
    },
  };
}

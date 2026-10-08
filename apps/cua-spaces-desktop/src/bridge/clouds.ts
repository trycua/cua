// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// New Space's address form and "Connect a cloud" (`spaces.add`, `clouds.*`),
// as the SwiftUI host runs them: the SDK's handshake with a Space at an
// address (`AppModel.addByAddress`), then the row it added; and the
// daemon's `cloud_status`, `cloud_test` and `cloud_connect` tools for your
// own clouds (AWS, Google Cloud, Modal). The sheet's rows and words are the
// core's; credentials never pass through here (each cloud signs in with
// its own CLI). A connect reads the clouds again, so New Space offers it.
import { object, optionalString, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { encode } from "./value";

export function cloudsMethods({ model }: BridgeContext): Handlers {
  return {
    "spaces.add": async (args) => {
      const before = new Set(model.spaces.map((s) => s.id));
      await model.addByAddress(string(args, "url"), optionalString(args, "token"), optionalString(args, "name"));
      const added = model.spaces.find((s) => !before.has(s.id));
      if (!added) throw Failure.failed("The Space was added but is not listed yet");
      return encode(added);
    },
    "clouds.status": () => model.backend.tool("cloud_status", {}),
    "clouds.test": (args) => model.backend.tool("cloud_test", object(args, "target")),
    "clouds.connect": async (args) => {
      const target = { ...object(args, "target"), make_default: args.makeDefault === true };
      const row = await model.backend.tool("cloud_connect", target);
      await model.cloud.refresh();
      return row;
    },
  };
}

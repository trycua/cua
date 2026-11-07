// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Cua Driver page's methods (`driver.*`, the web's `ops/driver.ts`).
// Only the Mac app carries Cua Driver inside Cua Spaces (an embedded child
// of cua-spacesd, with Cua Spaces' permissions). On Windows and Linux the
// driver runs in-process in cua-spacesd, so this host has no driver of its
// own: `driver.status` answers null, which keeps the page and its sidebar
// entry hidden, and the actions say why.
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

const NONE = "Cua Driver inside Cua Spaces comes with the Mac app";

export function driverMethods(_ctx: BridgeContext): Handlers {
  const no = () => {
    throw Failure.unsupported(NONE);
  };
  return {
    "driver.status": () => null,
    "driver.list": () => [],
    "driver.config": no,
    "driver.setConfig": no,
    "driver.test": no,
    "driver.pause": no,
    "driver.setup": no,
    "driver.setApp": no,
    // Cua Spaces asks for no screen or input permission here.
    "driver.permissions": () => null,
  };
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Packaged entry. Turns on Node's on-disk V8 compile cache before the main
// bundle loads, so the cache also covers main.cjs.
import * as NodeModule from "node:module";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";

try {
  // AppImage mounts at a new /tmp path every launch and the cache is keyed by
  // path, so it would only ever miss there. Linux /tmp is shared between
  // users, so use the per-user cache dir instead.
  if (!process.env.APPIMAGE) {
    const root =
      process.platform === "linux"
        ? process.env.XDG_CACHE_HOME || NodePath.join(NodeOS.homedir(), ".cache")
        : NodeOS.tmpdir();
    NodeModule.enableCompileCache?.(NodePath.join(root, "cua-spaces", "compile-cache"));
  }
} catch {
  // The cache is only a speedup.
}

require("./main.cjs");

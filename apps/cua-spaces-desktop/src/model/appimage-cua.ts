// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The daemon of an AppImage build. An AppImage runs from a FUSE mount that
// stays as long as any process started from it does, and the daemon outlives
// the app (it keeps the Spaces running), as it does with the SwiftUI app. Run
// from the mount, it would keep the AppImage mounted after the app quit. So
// the app starts it from a copy of the bundled `cua` in the user's data
// folder, one per AppImage build, as the CLI is copied onto PATH. A copy of
// another build is removed; a daemon still running from it keeps its file
// until it exits, and this app replaces that daemon as one of another build.
import { chmodSync, copyFileSync, mkdirSync, readdirSync, renameSync, rmSync, statSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

/** `$XDG_DATA_HOME/cua-spaces/daemon` (`~/.local/share/...` by default). */
export function appImageDaemonRoot(env: NodeJS.ProcessEnv = process.env): string {
  const data = env.XDG_DATA_HOME || path.join(os.homedir(), ".local", "share");
  return path.join(data, "cua-spaces", "daemon");
}

/**
 * The `cua` to start the daemon with: `bundled` copied under `root`, in a
 * folder named after this build (`version`, and the file's size and time,
 * which tell two builds of a version apart). Falls back to `bundled` when
 * the copy cannot be made.
 */
export function appImageCua(bundled: string, root: string, version: string): string {
  try {
    const st = statSync(bundled);
    const build = `${version}-${st.size}-${Math.trunc(st.mtimeMs)}`.replace(/[^\w.-]/g, "_");
    const dir = path.join(root, build);
    const copy = path.join(dir, path.basename(bundled));
    let current = false;
    try {
      current = statSync(copy).size === st.size;
    } catch {
      current = false;
    }
    if (!current) {
      mkdirSync(dir, { recursive: true });
      // Written aside and renamed in, so a daemon never starts a half-written file.
      const partial = `${copy}.${process.pid}.partial`;
      copyFileSync(bundled, partial);
      chmodSync(partial, 0o755);
      renameSync(partial, copy);
    }
    for (const other of readdirSync(root)) {
      if (other !== build) rmSync(path.join(root, other), { recursive: true, force: true });
    }
    return copy;
  } catch (error) {
    console.warn(`[cua-spaces] could not copy cua out of the AppImage, the daemon runs from it: ${error instanceof Error ? error.message : String(error)}`);
    return bundled;
  }
}

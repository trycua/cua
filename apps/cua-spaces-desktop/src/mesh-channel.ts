// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The update channel, recorded where the app's cua daemon and this
// machine's host read it (`<cua home>/app-update-channel`, cua_mesh::policy):
// the WireGuard-based mesh follows it once it is turned on for Beta. In this
// release the mesh is off on every channel unless CUA_MESH asks for it, so
// the file changes nothing yet.
import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import { cuaHome } from "./model/daemon";

/** cua_mesh::policy::CHANNEL_FILE. */
export const CHANNEL_FILE = "app-update-channel";

/** Writes `channel` to `<home>/app-update-channel` (atomically; a failure is only logged). */
export function recordUpdateChannel(channel: "stable" | "beta", home: string = cuaHome()): void {
  try {
    mkdirSync(home, { recursive: true });
    const file = path.join(home, CHANNEL_FILE);
    const tmp = `${file}.tmp`;
    writeFileSync(tmp, `${channel}\n`);
    renameSync(tmp, file);
  } catch (error) {
    console.warn("[cua-spaces] could not record the update channel:", error);
  }
}

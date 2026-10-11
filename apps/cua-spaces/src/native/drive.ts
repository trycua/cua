// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hasTauri } from './bridge';
import { agentsToolCall, driveRevealCall, type ToolCall } from './persistent';

/**
 * The Cua Volume's storage, mount and cache for Settings and the first run:
 * the daemon's `drive_*` Spaces tools through the shell's `agents_tool`,
 * plus the two things only the shell does (show a path inside the mounted
 * drive in the file manager; open System Settings' extensions pane). S3
 * keys only ever travel in `volume_storage_set`'s arguments.
 */
export interface DriveBridge {
  /** One daemon Spaces tool. */
  call: ToolCall;
  /** Shows a path inside the mounted drive (`drive_reveal`). */
  reveal(path: string): Promise<void>;
  /** Opens an allow-listed System Settings pane (`host_open_settings`). */
  openSettings(url: string): Promise<void>;
  /** The home folder, to show paths as `~/...` (null when unknown). */
  home(): Promise<string | null>;
}

export function createDriveBridge(): DriveBridge {
  if (hasTauri()) {
    const core = import('@tauri-apps/api/core');
    return {
      call: agentsToolCall(),
      reveal: driveRevealCall(),
      openSettings: async (url) => (await core).invoke<void>('host_open_settings', { url }),
      home: () =>
        import('@tauri-apps/api/path')
          .then(({ homeDir }) => homeDir())
          .catch(() => null),
    };
  }
  return {
    call: agentsToolCall(),
    reveal: driveRevealCall(),
    openSettings: () => Promise.reject(new Error('Opening System Settings needs the Cua Spaces app')),
    home: async () => null,
  };
}

/** A tool's JSON object answer, or null when the daemon cannot answer. */
export function answerOrNull<T>(call: ToolCall, tool: string, args?: Record<string, unknown>): Promise<T | null> {
  return call(tool, args).then(
    (r) => (r !== null && typeof r === 'object' && !Array.isArray(r) ? (r as T) : null),
    () => null,
  );
}

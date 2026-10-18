// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Sending host files into a Space, for the pop-out list window's Teleport drop
 * zone. Files land in the Space's `~/Downloads`.
 *
 * The shell's `send_files_to_space` verifies every file by sha256 IN THE SPACE
 * before it resolves, so a resolved promise here means the bytes are on the
 * target's filesystem — not merely that a request was accepted. Anything less
 * rejects, and the zone shows the rejection. Outside Tauri the bridge refuses
 * rather than pretending, so the browser/test build can never claim a send.
 */
import { hasTauri } from "./bridge";

/** Where a UI send lands. The MCP's `send_file` tool defaults to the same
 * directory but, unlike this, lets an agent override it. */
export const GUEST_DOWNLOADS_DIR = "~/Downloads";

export interface SentFile {
  name: string;
  /** Absolute path in the Space, with `~` already resolved there. */
  dest: string;
  bytes: number;
  /** The digest the Space computed for the landed file. */
  sha256: string;
}

export interface FileSendBridge {
  readonly isNative: boolean;
  /** Native open panel; resolves to the chosen host paths ([] if cancelled). */
  pickFiles(): Promise<string[]>;
  /** Send host files to the Space's ~/Downloads. Rejects unless all landed. */
  sendFiles(spaceId: string, paths: string[]): Promise<SentFile[]>;
}

export function createFallbackFileSendBridge(): FileSendBridge {
  const refuse = async (): Promise<never> => {
    throw new Error("sending files needs the Cua Spaces app");
  };
  return { isNative: false, pickFiles: async () => [], sendFiles: refuse };
}

export function createTauriFileSendBridge(): FileSendBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    pickFiles: async () => {
      // The dialog plugin's own IPC command, called directly so the pop-out
      // does not pull in a second copy of its JS wrapper.
      const chosen = await invoke<string[] | string | null>("plugin:dialog|open", {
        options: { multiple: true, directory: false, title: "Teleport" },
      });
      if (!chosen) return [];
      return Array.isArray(chosen) ? chosen : [chosen];
    },
    sendFiles: (spaceId, paths) =>
      invoke<SentFile[]>("send_files_to_space", { spaceId, paths }),
  };
}

export function createFileSendBridge(): FileSendBridge {
  return hasTauri() ? createTauriFileSendBridge() : createFallbackFileSendBridge();
}

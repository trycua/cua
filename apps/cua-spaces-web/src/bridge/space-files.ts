// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space's drop well, as the SwiftUI detail's `TeleportDropZone` runs it:
 * "Send file…" picks files with the host's own picker, files dropped on the
 * well reach the host by name (it reads their paths from the drag), and
 * either list is sent into the Space (`spaces.sendFiles`). An app bundle
 * dropped there opens Teleport at that app, with the files dropped beside
 * it. The line under the well is the core's (`transfer.dropSendingText`,
 * `transfer.dropSentText`).
 */

import { useContext, useEffect, useState } from "react";
import { BridgeContext } from "./BridgeProvider";
import type { DataAdapter } from "./adapter";
import type { CoreClient } from "./core";
import type { SentFileInfo } from "./ops/space-detail";
import { teleportStore } from "./teleport";

/** The line under the well: what the last drop did. */
export type DropStatus = { kind: "working" | "done" | "failed"; text: string } | null;

const words = (e: unknown) => (e instanceof Error ? e.message : String(e));
const isApp = (path: string) => /\.app\/?$/i.test(path);

/** "Sending notes.txt…" (`transfer::drop_sending_text`). */
export function dropSendingText(core: CoreClient, paths: string[]): string {
  return core.tryCall<string>("transfer.dropSendingText", { paths }) ?? `Sending ${paths.length === 1 ? (paths[0]!.split("/").pop() ?? paths[0]) : `${paths.length} files`}…`;
}

/** "notes.txt (1 KB) verified in …" (`transfer::drop_sent_text`). */
export function dropSentText(core: CoreClient, files: SentFileInfo[]): string {
  return core.tryCall<string>("transfer.dropSentText", { files }) ?? files.map((f) => f.name).join(", ");
}

/** Sends `paths` into the Space, reporting each step of the well's line. */
export async function sendFiles(adapter: DataAdapter, core: CoreClient, spaceId: string, paths: string[], report: (s: DropStatus) => void): Promise<void> {
  if (!paths.length) return;
  report({ kind: "working", text: dropSendingText(core, paths) });
  try {
    const files = await adapter.call("spaces.sendFiles", { spaceId, paths });
    report({ kind: "done", text: dropSentText(core, files) });
  } catch (e) {
    report({ kind: "failed", text: words(e) });
  }
}

/** What a drop on the well means: an app to teleport (with the files
 * beside it), or files to send. */
export function splitDrop(paths: string[]): { app: string | null; files: string[] } {
  const app = paths.find(isApp) ?? null;
  return { app, files: paths.filter((p) => p !== app) };
}

/**
 * The names the host left on a Space's address when the person dropped apps
 * or files on that Space's tile in the notch (`?dropped=["Slack.app", ...]`):
 * the same names a drop on the well carries, which the host resolves to the
 * paths of that drop. Anything else is no drop.
 */
export function parseDropped(search: unknown): string[] | undefined {
  if (!Array.isArray(search) || search.length === 0 || search.length > 200) return undefined;
  return search.every((n) => typeof n === "string" && n !== "") ? (search as string[]) : undefined;
}

export interface SpaceFilesHook {
  status: DropStatus;
  /** "Send file…": the host's picker, then the transfer. */
  choose(): Promise<void>;
  /** Files dropped on the well, by name. */
  drop(names: string[]): Promise<void>;
}

/** The drop well of `space`. */
export function useSpaceFiles(space: { id: string; name: string }): SpaceFilesHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("useSpaceFiles needs a <BridgeProvider> above it");
  const [status, setStatus] = useState<{ spaceId: string; status: DropStatus } | null>(null);
  useEffect(() => setStatus(null), [space.id]);
  const report = (s: DropStatus) => setStatus({ spaceId: space.id, status: s });
  const { core, ready } = ctx;
  return {
    status: status?.spaceId === space.id ? status.status : null,
    choose: async () => {
      try {
        const { adapter } = await ready;
        await sendFiles(adapter, core, space.id, await adapter.call("spaces.chooseFiles", {}), report);
      } catch (e) {
        report({ kind: "failed", text: words(e) });
      }
    },
    drop: async (names) => {
      try {
        const store = await ready;
        const { adapter } = store;
        const { app, files } = splitDrop(await adapter.call("spaces.droppedFiles", { names }));
        if (app) {
          const entry = await adapter.call("teleport.entryForPath", { path: app });
          teleportStore(store).open(space, entry, files);
          return;
        }
        await sendFiles(adapter, core, space.id, files, report);
      } catch (e) {
        report({ kind: "failed", text: words(e) });
      }
    },
  };
}

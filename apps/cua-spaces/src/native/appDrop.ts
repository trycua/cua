// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * An app dropped on a Space (a `.app` from Finder or the Dock, on a Space
 * tile, the Space's window or its viewer) opens "Teleport an app…" for that
 * app, preselected, with any files dropped alongside it. Anything else is
 * not an app drop, and the caller keeps its file transfer.
 */
import type { LocalApp } from "../model/teleport";
import { createTeleportAppsBridge, type TeleportAppsBridge } from "./teleportApps";
import { createTeleportBridge, type TeleportBridge } from "./teleport";

export interface AppDropTarget {
  id: string;
  name: string;
}

/** Opens the picker for a resolved app (shell window or in-portal sheet). */
export type OpenAppTeleport = (
  spaceId: string,
  spaceName: string,
  app: LocalApp | null,
  entry: Record<string, unknown> | null,
  files: string[],
) => void;

/** Cheap check before any async work: does the drop carry an app bundle? */
export function hasAppPath(paths: readonly string[]): boolean {
  return paths.some((p) => /\.(app|desktop|lnk)\/?$/i.test(p));
}

/**
 * Handles an app drop onto `space`. Resolves to true when the drop carried
 * an app (the picker was asked to open), false when it did not (the caller
 * handles files and folders as before).
 */
export async function handleAppDrop(
  paths: readonly string[],
  space: AppDropTarget,
  open: OpenAppTeleport,
  apps: TeleportAppsBridge = createTeleportAppsBridge(),
): Promise<boolean> {
  if (!hasAppPath(paths)) return false;
  const drop = await apps.parseDrop([...paths]);
  const bundle = drop.apps[0];
  if (drop.kind !== "app" || !bundle) return false;
  try {
    const entry = await apps.entryForPath(bundle);
    open(space.id, space.name, { id: entry.id, name: entry.name }, JSON.parse(entry.json), drop.files);
  } catch {
    // Unreadable bundle: the picker still opens, on the full catalog.
    open(space.id, space.name, null, null, drop.files);
  }
  return true;
}

/** The viewer's hook: opens the shell's picker window for `space`. */
export function handleViewerAppDrop(
  paths: readonly string[],
  space: AppDropTarget,
  teleport: TeleportBridge = createTeleportBridge(),
): Promise<boolean> {
  return handleAppDrop(paths, space, (spaceId, spaceName, app, entry, files) => {
    void teleport.openPicker({ spaceId, spaceName, app, entry, files }).catch(() => {});
  });
}

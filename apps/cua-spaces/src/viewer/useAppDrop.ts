// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect } from "react";

import { handleViewerAppDrop, hasAppPath } from "../native/appDrop";
import { hasTauri } from "../native/bridge";

/**
 * The viewer's app-drop hook: an app bundle dropped anywhere on a Space's
 * window (Finder, Dock) opens "Teleport an app…" for it, preselected.
 * Drops without an app are left alone: the viewer's own file drop (sharing
 * files and folders) handles those, and should ignore drops where
 * {@link hasAppPath} is true.
 */
export function useAppDropTeleport(space: { id: string; name: string } | null): void {
  const id = space?.id;
  const name = space?.name;
  useEffect(() => {
    if (!id || !name || !hasTauri()) return;
    let cancelled = false;
    let stop: (() => void) | null = null;
    void import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type !== "drop" || !hasAppPath(event.payload.paths)) return;
          void handleViewerAppDrop(event.payload.paths, { id, name });
        }),
      )
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [id, name]);
}

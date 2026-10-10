// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback } from "react";

import { isNativeHost, useBridge, useNotificationPosts, type SystemNote } from "@/bridge";
import { toast } from "@/components/ui/toast";

/**
 * Shows what the core says to announce from the notifications feed (each
 * entry once, none of the backlog, a summary after three) as toasts. The
 * native hosts (the SwiftUI app, the Electron shell) post their own system
 * notifications, so their pages don't.
 */
export function useNotificationToasts(): void {
  const { mode } = useBridge();
  const show = useCallback(
    (note: SystemNote) => {
      if (!isNativeHost(mode)) toast(note.title, { description: note.body || undefined });
    },
    [mode],
  );
  useNotificationPosts(show);
}

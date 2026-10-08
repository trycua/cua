// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useSpaces } from "@/bridge";

/**
 * The host's last read of your Spaces failed: one line above every page,
 * while the rows it listed before stay (the SwiftUI app's
 * `SpacesDiscoveryNotice`). It goes with the next read that works.
 */
export function SpacesNotice() {
  const { listNotice } = useSpaces();
  if (!listNotice) return null;
  return (
    <p role="status" data-spaces-discovery-error="" className="shrink-0 border-b px-4 py-2.5 text-xs text-destructive">
      {listNotice}
    </p>
  );
}

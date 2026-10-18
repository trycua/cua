// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useSpaces } from "@/bridge";

/**
 * The host's last read of your Spaces failed: one line above every page,
 * while the rows it listed before stay (the SwiftUI app's
 * `SpacesDiscoveryNotice`). It goes with the next read that works. The
 * host's standing notice (another app keeps its own daemon) shows the same
 * way while it lasts.
 */
export function SpacesNotice() {
  const { listNotice, hostNotice } = useSpaces();
  if (!listNotice && !hostNotice) return null;
  return (
    <>
      {hostNotice ? (
        <p role="status" data-host-notice="" className="shrink-0 border-b px-4 py-2.5 text-xs text-muted-foreground">
          {hostNotice}
        </p>
      ) : null}
      {listNotice ? (
        <p role="status" data-spaces-discovery-error="" className="shrink-0 border-b px-4 py-2.5 text-xs text-destructive">
          {listNotice}
        </p>
      ) : null}
    </>
  );
}

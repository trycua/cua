// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Button } from "@/components/ui/button";

/**
 * "Set up later", top right in the title bar. It keeps clear of the window
 * controls overlay (Electron's titleBarOverlay on Windows and Linux), which
 * otherwise covers it and takes its clicks.
 */
export function SetUpLater({ onClick }: { onClick: () => void }) {
  return (
    <Button
      variant="ghost"
      size="sm"
      className="app-no-drag absolute -top-9 text-muted-foreground"
      style={{ right: "calc(var(--titlebar-right-inset) + 1rem)" }}
      onClick={onClick}
    >
      Set up later
    </Button>
  );
}

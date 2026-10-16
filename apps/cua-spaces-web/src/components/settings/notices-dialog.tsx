// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { useSession } from "@/bridge";
import { Markdown } from "@/components/markdown";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";

/**
 * The third-party notices this UI ships with (the repository's
 * THIRD_PARTY_NOTICES.md: T3 Code's licence and the npm dependencies),
 * loaded only when opened. About's Acknowledgements opens it, as the
 * SwiftUI app's opens its bundled notices.
 */
export function NoticesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { openExternal } = useSession();
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    if (!open || text !== null) return;
    void import("../../../../../THIRD_PARTY_NOTICES.md?raw").then(
      (m) => setText(m.default.replace(/<!--[\s\S]*?-->\n?/g, "")),
      () => setText("The third-party notices are missing from this build (THIRD_PARTY_NOTICES.md)."),
    );
  }, [open, text]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[8vh] flex max-h-[80vh] w-[min(640px,calc(100vw-2rem))] flex-col" data-notices>
        <DialogTitle className="border-b px-5 py-3 text-[15px] font-semibold">Acknowledgements</DialogTitle>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4 select-text">
          {text === null ? <p className="text-[13px] text-muted-foreground">Loading…</p> : <Markdown text={text} onOpenLink={(url) => void openExternal(url)} />}
        </div>
        <div className="flex justify-end border-t bg-muted/60 px-5 py-3">
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { useSession } from "@/bridge";
import { Markdown } from "@/components/markdown";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";

/** Where the Mac app serves its own bundled notices (`WebUISchemeHandler`, from the app bundle, never the network). */
export const MAC_APP_NOTICES_PATH = "/__app/THIRD_PARTY_NOTICES.md";

/** In the Mac app's window (its host script sets `data-host="macos"`). */
const inMacApp = () => typeof document !== "undefined" && document.documentElement.dataset.host === "macos";

const stripComments = (t: string) => t.replace(/<!--[\s\S]*?-->\n?/g, "");

/** Markdown tables as lists (the Markdown view draws no tables): each row's cells, header and rule dropped. */
export const tablesAsLists = (t: string) => {
  const out: string[] = [];
  let inTable = false;
  for (const line of t.split("\n")) {
    if (!/^\s*\|.*\|\s*$/.test(line)) {
      inTable = false;
      out.push(line);
      continue;
    }
    if (/^\s*\|[\s:|-]+\|\s*$/.test(line)) continue;
    if (!inTable) {
      inTable = true; // the header row
      continue;
    }
    const cells = line.trim().slice(1, -1).split("|").map((c) => c.trim()).filter(Boolean);
    out.push(`- ${cells.join(" · ")}`);
  }
  return out.join("\n");
};

/**
 * The third-party notices, loaded only when opened: in the Mac app, first
 * the app's own (its bundled THIRD_PARTY_NOTICES.md: the notch's sources and
 * the Swift packages), then this UI's (the repository's
 * THIRD_PARTY_NOTICES.md: T3 Code's licence and the npm dependencies).
 * About's Acknowledgements opens it.
 */
export function NoticesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { openExternal } = useSession();
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    if (!open || text !== null) return;
    const app = inMacApp()
      ? fetch(MAC_APP_NOTICES_PATH).then((r) => (r.ok ? r.text() : null)).catch(() => null)
      : Promise.resolve(null);
    const web = import("../../../../../legal/THIRD_PARTY_NOTICES.md?raw").then((m) => m.default as string, () => null);
    void Promise.all([app, web]).then(([appText, webText]) => {
      const parts = [appText && `# Cua Spaces for Mac\n\n${tablesAsLists(stripComments(appText).replace(/^# .*\n+/, ""))}`, webText && stripComments(webText)].filter(Boolean);
      setText(parts.length ? parts.join("\n\n") : "The third-party notices are missing from this build (THIRD_PARTY_NOTICES.md).");
    });
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

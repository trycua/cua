// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { useSession } from "@/bridge";
import { Markdown } from "@/components/markdown";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { useT } from "@/i18n";

/** Where the Mac app serves its own bundled notices (`WebUISchemeHandler`, from the app bundle, never the network). */
export const MAC_APP_NOTICES_PATH = "/__app/THIRD_PARTY_NOTICES.md";

/** In the Mac app's window (its host script sets `data-host="macos"`). */
const inMacApp = () => typeof document !== "undefined" && document.documentElement.dataset.host === "macos";

const stripComments = (md: string) => md.replace(/<!--[\s\S]*?-->\n?/g, "");

/** Markdown tables as lists (the Markdown view draws no tables): each row's cells, header and rule dropped. */
export const tablesAsLists = (md: string) => {
  const out: string[] = [];
  let inTable = false;
  for (const line of md.split("\n")) {
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
    const cells = line
      .trim()
      .slice(1, -1)
      .split("|")
      .map((c) => c.trim())
      .filter(Boolean);
    out.push(`- ${cells.join(", ")}`);
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
  const t = useT();
  const { openExternal } = useSession();
  // What was read, worded when drawn, so the headings and the missing note follow the language.
  const [read, setRead] = useState<{ app: string | null; web: string | null } | null>(null);
  useEffect(() => {
    if (!open || read !== null) return;
    const app = inMacApp()
      ? fetch(MAC_APP_NOTICES_PATH)
          .then((r) => (r.ok ? r.text() : null))
          .catch(() => null)
      : Promise.resolve(null);
    const web = import("../../../../../legal/THIRD_PARTY_NOTICES.md?raw").then(
      (m) => m.default as string,
      () => null,
    );
    void Promise.all([app, web]).then(([appText, webText]) => {
      setRead({ app: appText && tablesAsLists(stripComments(appText).replace(/^# .*\n+/, "")), web: webText && stripComments(webText) });
    });
  }, [open, read]);
  const parts = read ? [read.app && `# ${t("settings.notices.mac_heading")}\n\n${read.app}`, read.web].filter(Boolean) : [];
  const text = read ? (parts.length ? parts.join("\n\n") : t("settings.notices.missing")) : null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[8vh] flex max-h-[80vh] w-[min(640px,calc(100vw-2rem))] flex-col" data-notices>
        <DialogTitle /* design-ok: full-width dialog header rule */ className="border-b px-5 py-3 text-[15px] font-semibold">
          {t("settings.notices.title")}
        </DialogTitle>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4 select-text">
          {text === null ? (
            <p className="text-[13px] text-muted-foreground">{t("settings.notices.loading")}</p>
          ) : (
            <Markdown text={text} onOpenLink={(url) => void openExternal(url)} />
          )}
        </div>
        <div /* design-ok: full-width dialog footer */ className="flex justify-end border-t bg-muted/60 px-5 py-3">
          <Button onClick={() => onOpenChange(false)}>{t("settings.notices.done")}</Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

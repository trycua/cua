// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useLinkNotice, useSession } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";

/**
 * The one-time notice for machines that saw the older "anonymous" usage-data
 * wording: usage data is now linked to the signed-in account through a
 * random ID. The host says when it is due and owns the words. Only the two
 * buttons close it, so an Escape or a click outside can't count as a choice.
 */
export function UsageLinkNotice() {
  const { notice, busy, error, keep, turnOff } = useLinkNotice();
  const { openExternal } = useSession();
  if (!notice) return null;
  return (
    <Dialog open onOpenChange={() => undefined} disablePointerDismissal>
      <DialogPopup className="top-[14vh] w-[min(460px,calc(100vw-2rem))]" data-usage-link-notice="">
        <div className="px-6 pt-5 pb-4">
          <DialogTitle className="text-[15px] font-semibold">{notice.title}</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{notice.body}</DialogDescription>
          <button
            type="button"
            className="mt-2 text-xs text-foreground underline-offset-2 hover:underline"
            onClick={() => void openExternal(notice.docsUrl).catch(() => undefined)}
          >
            {notice.detailsLabel}
          </button>
          {error ? (
            <p role="alert" className="mt-2 text-xs text-destructive">
              {error}
            </p>
          ) : null}
          <div className="mt-4 flex justify-end gap-2">
            <Button type="button" variant="outline" disabled={busy} onClick={() => void turnOff()}>
              {notice.turnOffLabel}
            </Button>
            <Button type="button" autoFocus disabled={busy} onClick={() => void keep()}>
              {notice.okLabel}
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

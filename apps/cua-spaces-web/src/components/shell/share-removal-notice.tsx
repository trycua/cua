// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";

import { useSession, useShareRemovalNote } from "@/bridge";
import { Button } from "@/components/ui/button";

/**
 * Sharing is for teams now (CUA-1883): the owner sees once, in one line,
 * who can no longer open a shared computer, with "Invite to your team".
 * The words are the app core's (`share.removalNote`); closing it tells the
 * host it was seen, so it does not come back.
 */
export function ShareRemovalNotice() {
  const { data: session, openExternal } = useSession();
  const { note, dismiss } = useShareRemovalNote(Boolean(session?.signedIn));
  if (!note) return null;
  return (
    <div role="status" data-share-removal="" className="flex shrink-0 items-center gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
      <span className="min-w-0 flex-1">{note.text}</span>
      <Button size="sm" variant="outline" data-share-removal-invite="" onClick={() => void openExternal(note.action.url).catch(() => {})}>
        {note.action.label}
      </Button>
      <Button size="icon-sm" variant="ghost" aria-label={note.dismiss} data-share-removal-dismiss="" onClick={() => void dismiss()}>
        <XIcon className="size-3.5" />
      </Button>
    </div>
  );
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";
import { useSyncExternalStore } from "react";

import { useRelayUsageView, useSession } from "@/bridge";
import { Button } from "@/components/ui/button";

const DISMISSED_KEY = "cua.relayAllowance.dismissed";

function readDismissed(): string | null {
  try {
    return globalThis.localStorage?.getItem(DISMISSED_KEY) ?? null;
  } catch {
    return null;
  }
}

const listeners = new Set<() => void>();
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/**
 * Whether the allowance notice is on screen now. The update indicator
 * (`update-indicator.tsx`) steps back to an icon while it is, so the two
 * prompts are never both open at once.
 */
export function useRelayAllowanceShowing(): boolean {
  const notice = useRelayUsageView()?.notice;
  const dismissed = useSyncExternalStore(subscribe, readDismissed, () => null);
  return Boolean(notice) && dismissed !== notice?.dismissKey;
}

/**
 * The relay allowance for the month is used up: one line above every page
 * with Upgrade, until dismissed (for that month and reason: the core's
 * `dismissKey`). Nothing stops at the limit, so it only says so.
 */
export function RelayAllowanceNotice() {
  const notice = useRelayUsageView()?.notice;
  const { openExternal } = useSession();
  const dismissed = useSyncExternalStore(subscribe, readDismissed, () => null);
  if (!notice || dismissed === notice.dismissKey) return null;
  const dismiss = () => {
    try {
      globalThis.localStorage?.setItem(DISMISSED_KEY, notice.dismissKey);
    } catch {
      // Not kept: it shows again next launch.
    }
    for (const l of [...listeners]) l();
  };
  const upgradeUrl = notice.upgradeUrl;
  return (
    <div role="status" data-relay-allowance="" className="flex shrink-0 items-center gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
      <span className="min-w-0 flex-1">{notice.text}</span>
      {upgradeUrl ? (
        <Button size="sm" variant="outline" data-relay-upgrade="" onClick={() => void openExternal(upgradeUrl).catch(() => {})}>
          {notice.upgrade}
        </Button>
      ) : null}
      <Button size="icon-sm" variant="ghost" aria-label={notice.dismiss} data-relay-dismiss="" onClick={dismiss}>
        <XIcon className="size-3.5" />
      </Button>
    </div>
  );
}

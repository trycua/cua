// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";
import { useState } from "react";

import { useRelayUsage, useSession, type RelayUsage } from "@/bridge";
import { Button } from "@/components/ui/button";

const DISMISSED_KEY = "cua.relayAllowance.dismissed";

/** One dismissal per month and reason: a new month, or another limit reached, shows it again. */
const dismissKey = (u: RelayUsage) => `${u.period}:${[...u.reasons].sort().join(",")}`;

function readDismissed(): string | null {
  try {
    return globalThis.localStorage?.getItem(DISMISSED_KEY) ?? null;
  } catch {
    return null;
  }
}

/**
 * The relay allowance for the month is used up: one line above every page
 * with Upgrade, until dismissed. Nothing stops at the limit, so it only
 * says so.
 */
export function RelayAllowanceNotice() {
  const usage = useRelayUsage();
  const { openExternal } = useSession();
  const [dismissed, setDismissed] = useState(readDismissed);
  if (!usage?.softLimitReached || dismissed === dismissKey(usage)) return null;
  const dismiss = () => {
    const key = dismissKey(usage);
    try {
      globalThis.localStorage?.setItem(DISMISSED_KEY, key);
    } catch {
      // Not kept: it shows again next launch.
    }
    setDismissed(key);
  };
  return (
    <div role="status" data-relay-allowance="" className="flex shrink-0 items-center gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
      <span className="min-w-0 flex-1">
        You've used your {usage.plan === "free" ? "free " : ""}relay allowance for this month. Your Spaces keep working. Upgrade for more.
      </span>
      {usage.upgradeUrl ? (
        <Button size="sm" variant="outline" data-relay-upgrade="" onClick={() => void openExternal(usage.upgradeUrl!).catch(() => {})}>
          Upgrade
        </Button>
      ) : null}
      <Button size="icon-sm" variant="ghost" aria-label="Dismiss" data-relay-dismiss="" onClick={dismiss}>
        <XIcon className="size-3.5" />
      </Button>
    </div>
  );
}

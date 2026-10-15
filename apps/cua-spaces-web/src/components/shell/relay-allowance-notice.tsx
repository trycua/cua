// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";
import { useState } from "react";

import { useBridge, useRelayUsageView, useSession } from "@/bridge";
import { Button } from "@/components/ui/button";

const DISMISSED_KEY = "cua.relayAllowance.dismissed";

function readDismissed(): string | null {
  try {
    return globalThis.localStorage?.getItem(DISMISSED_KEY) ?? null;
  } catch {
    return null;
  }
}

/**
 * The relay allowance for the month is used up: one line above every page
 * with Upgrade, until dismissed (for that month and reason: the core's
 * `dismissKey`). Nothing stops at the limit, so it only says so.
 */
export function RelayAllowanceNotice() {
  const notice = useRelayUsageView()?.notice;
  const { openExternal } = useSession();
  const { data } = useBridge();
  const [dismissed, setDismissed] = useState(readDismissed);
  if (!notice || dismissed === notice.dismissKey) return null;
  const dismiss = () => {
    try {
      globalThis.localStorage?.setItem(DISMISSED_KEY, notice.dismissKey);
    } catch {
      // Not kept: it shows again next launch.
    }
    setDismissed(notice.dismissKey);
  };
  const upgradeUrl = notice.upgradeUrl;
  return (
    <div role="status" data-relay-allowance="" className="flex shrink-0 items-center gap-3 border-b px-4 py-2 text-xs text-muted-foreground">
      <span className="min-w-0 flex-1">{notice.text}</span>
      {upgradeUrl ? (
        <Button size="sm" variant="outline" data-relay-upgrade="" onClick={() => {
            // The app side of the paid funnel; a fixed word, dropped while usage data is off.
            void data?.call("telemetry.track", { signals: [{ type: "feature", feature: "upgrade_open" }] }).catch(() => {});
            void openExternal(upgradeUrl).catch(() => {});
          }}>
          {notice.upgrade}
        </Button>
      ) : null}
      <Button size="icon-sm" variant="ghost" aria-label={notice.dismiss} data-relay-dismiss="" onClick={dismiss}>
        <XIcon className="size-3.5" />
      </Button>
    </div>
  );
}

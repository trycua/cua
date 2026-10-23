// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect } from "react";

import { usePlanLimitNotice, useSession } from "@/bridge";
import { setErrorAction } from "@/components/ui/toast";

/**
 * Puts Upgrade on the toast of a failed action when the failure is the
 * relay refusing a new live session over the plan's cap ("You're using 5 of
 * 5 live sessions on Pro. Close one or upgrade to Teams."). The sentence is
 * the error's own; the button opens the plan's page on the account app.
 * Renders nothing.
 */
export function PlanLimitToastAction() {
  const noticeFor = usePlanLimitNotice();
  const { openExternal } = useSession();
  useEffect(() => {
    setErrorAction((text) => {
      const notice = noticeFor(text);
      const url = notice?.upgradeUrl;
      if (!notice || !url) return null;
      return { label: notice.upgrade, onClick: () => void openExternal(url).catch(() => {}) };
    });
    return () => setErrorAction(null);
  }, [noticeFor, openExternal]);
  return null;
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect } from "react";

import { usePlanLimitNotice, usePlanLimitTelemetry, useSession } from "@/bridge";
import { setErrorAction } from "@/components/ui/toast";

/**
 * Puts Upgrade on the toast of a failed action when the failure is the
 * relay refusing a new live session over the plan's cap ("You're using 5 of
 * 5 live sessions on Pro. Close one or upgrade to Teams."). The sentence is
 * the error's own; the button opens the plan's page on the account app.
 * Renders nothing.
 *
 * Usage data (CUA-1881): the refusal shown is `cua_plan_limit_shown` and
 * Upgrade `cua_upgrade_link_opened`, at `app_create` (a Space create's
 * toast) or `app_connect` (opening or starting one); the link says so too
 * (`?from=`).
 */
export function PlanLimitToastAction() {
  const noticeFor = usePlanLimitNotice();
  const plans = usePlanLimitTelemetry();
  const { openExternal } = useSession();
  useEffect(() => {
    setErrorAction((text, group) => {
      const notice = noticeFor(text);
      const url = notice?.upgradeUrl;
      if (!notice || !url) return null;
      const surface = group === "create" ? "app_create" : "app_connect";
      const facts = { kind: notice.kind, plan: notice.plan };
      plans.shown(surface, facts);
      return {
        label: notice.upgrade,
        onClick: () => {
          plans.opened(surface, facts);
          void openExternal(plans.link(url, surface)).catch(() => {});
        },
      };
    });
    return () => setErrorAction(null);
  }, [noticeFor, openExternal, plans]);
  return null;
}

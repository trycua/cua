// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Spaces' two permissions, Screen Recording and Accessibility, with
 * Allow and their live status (`useAppPermissions`). One component for every
 * place that shows them: the first run's permissions step and next steps
 * (`hero`, drawn on the spaces.cua.ai hero) and the Cua Driver page and
 * This Mac (`settings`, the app's grouped rows). Cua Driver uses these same
 * grants, so there is no second app to allow.
 */

import { CheckIcon } from "lucide-react";
import type { ReactNode } from "react";

import type { AppPermissionsState } from "@/bridge/app-permissions";
import { CARD, GLASS_CHIP, GREY } from "@/components/first-run-new/hero";
import { Button } from "@/components/ui/button";
import { GroupedSection, SettingRow } from "@/components/ui/grouped";
import { toastError } from "@/components/ui/toast";
import { allowHint, permissionWords } from "@/lib/app-permissions";
import { cn } from "@/lib/utils";

export function AppPermissionList({
  perms,
  variant,
  title = "Permissions",
  footer,
  inset,
  className,
}: {
  perms: Pick<AppPermissionsState, "data" | "allow">;
  variant: "hero" | "settings";
  /** The settings section's heading. */
  title?: string;
  /** Under the rows; the hero's default says what Allow does. */
  footer?: ReactNode;
  /** Hero rows inside another card (the next steps' row): no card of their own. */
  inset?: boolean;
  className?: string;
}) {
  const data = perms.data;
  if (!data?.permissions.length) return null;
  const allow = (id: Parameters<typeof perms.allow>[0]) => void perms.allow(id).catch(toastError("Couldn't open System Settings"));

  if (variant === "settings") {
    return (
      <GroupedSection title={title} className={cn("mb-8", className)} data-app-permissions="" footer={footer}>
        {data.permissions.map((p) => {
          const words = permissionWords(p.id, data.appName);
          return (
            <SettingRow
              key={p.id}
              data-app-permission={p.id}
              data-granted={p.granted ? "" : undefined}
              label={words.title}
              help={`${words.sub}.`}
              value={p.granted ? "On" : "Missing"}
              control={
                p.granted || !p.settingsUrl ? undefined : (
                  <Button variant="outline" size="sm" onClick={() => allow(p.id)}>
                    Allow
                  </Button>
                )
              }
            />
          );
        })}
      </GroupedSection>
    );
  }

  return (
    <div className={className} data-app-permissions="">
      <div className={inset ? undefined : CARD}>
        {data.permissions.map((p) => {
          const words = permissionWords(p.id, data.appName);
          return (
            <div key={p.id} className="flex min-h-[52px] items-center gap-3 px-3 py-[7px]" data-app-permission={p.id} data-granted={p.granted ? "" : undefined}>
              <span className="min-w-0 flex-1">
                <span className="block text-[15px] leading-5 font-medium text-white">{words.title}</span>
                <span className={cn("block text-[13.5px] leading-[19px] font-medium", GREY)}>{words.sub}</span>
              </span>
              {p.granted ? (
                <span className="flex items-center gap-1 pr-1 text-[13.5px] font-medium text-white" role="status">
                  <CheckIcon aria-hidden className="size-3.5" strokeWidth={2.6} /> Allowed
                </span>
              ) : p.settingsUrl ? (
                <button type="button" className={cn(GLASS_CHIP, "h-[30px] shrink-0 px-3.5 text-[13.5px]")} onClick={() => allow(p.id)}>
                  Allow
                </button>
              ) : null}
            </div>
          );
        })}
      </div>
      <p className={cn(inset ? "mt-1 px-3 pb-1" : "mt-3 px-1", "max-w-[480px] text-[13.5px] leading-[19px] font-medium", GREY)}>{footer ?? allowHint(data.appName)}</p>
    </div>
  );
}

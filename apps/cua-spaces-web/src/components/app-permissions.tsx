// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Spaces' two permissions, Screen Recording and Accessibility, with
 * Allow and their live status (`useAppPermissions`): On with a check, a
 * spinner while the switch is waited for, or what System Settings shows. One component for every
 * place that shows them: the first run's permissions step and next steps
 * (`hero`, drawn on the spaces.cua.ai hero) and the Cua Driver page and
 * This Mac (`settings`, the app's grouped rows). Cua Driver uses these same
 * grants, so there is no second app to allow.
 */

import { CheckIcon, LoaderCircleIcon } from "lucide-react";
import type { ReactNode } from "react";

import type { AppPermissionsState } from "@/bridge/app-permissions";
import type { LocalNetworkState } from "@/bridge/local-network";
import { CARD, GLASS_CHIP, GREY } from "@/components/first-run-new/hero";
import { Button } from "@/components/ui/button";
import { GroupedSection, SettingRow } from "@/components/ui/grouped";
import { toastError } from "@/components/ui/toast";
import {
  allowHint,
  localNetworkButton,
  localNetworkStatus,
  localNetworkWords,
  permissionState,
  permissionStatus,
  permissionWords,
} from "@/lib/app-permissions";
import type { DriverPermission } from "@/bridge/ops/driver";
import { cn } from "@/lib/utils";

/** What every permission list needs from `useAppPermissions`; `waiting` is optional for callers that draw a fixed state. */
export type PermissionsView = Pick<AppPermissionsState, "data" | "allow"> & { waiting?: boolean };

/**
 * One row's live status beside Allow: a check and "On" once granted, a spinner while the switch is being waited for
 * (after Allow, and after the app reopens), else what System Settings shows. Plain text, no badge.
 */
export function PermissionStatus({
  permission,
  waiting,
  className,
}: {
  permission: Pick<DriverPermission, "granted" | "state">;
  waiting: boolean;
  className?: string;
}) {
  const status = permissionStatus(permission, waiting);
  if (!status) return null;
  return (
    <span
      className={cn("flex shrink-0 items-center gap-1", className)}
      role="status"
      aria-live="polite"
      data-permission-status={status.checking ? "checking" : permissionState(permission)}
    >
      {status.done ? (
        <CheckIcon aria-hidden className="size-3.5" strokeWidth={2.6} />
      ) : status.checking ? (
        <LoaderCircleIcon aria-hidden className="size-3.5 animate-spin" />
      ) : null}
      {status.label}
    </span>
  );
}

/**
 * Local Network on a Mac that runs macOS Spaces (`useLocalNetwork`), drawn as one more hero row: Allow makes macOS ask
 * (its own question, right after the step's words say why), then the answer shows live; once macOS was told no, the
 * button opens the Local Network settings.
 */
export function LocalNetworkRow({ net, appName }: { net: Pick<LocalNetworkState, "granted" | "waiting" | "denied" | "allow">; appName: string }) {
  const words = localNetworkWords(appName);
  const status = localNetworkStatus(net);
  return (
    <div className="flex min-h-[52px] items-center gap-3 px-3 py-[7px]" data-app-permission="local-network" data-granted={net.granted ? "" : undefined}>
      <span className="min-w-0 flex-1">
        <span className="block text-[15px] leading-5 font-medium text-white">{words.title}</span>
        <span className={cn("block text-[13.5px] leading-[19px] font-medium", GREY)}>{words.sub}</span>
      </span>
      {status ? (
        <span
          className={cn("flex shrink-0 items-center gap-1 pr-1 text-[13.5px] font-medium", net.granted ? "text-white" : GREY)}
          role="status"
          aria-live="polite"
          data-permission-status={status.checking ? "checking" : net.granted ? "on" : "off"}
        >
          {status.done ? (
            <CheckIcon aria-hidden className="size-3.5" strokeWidth={2.6} />
          ) : status.checking ? (
            <LoaderCircleIcon aria-hidden className="size-3.5 animate-spin" />
          ) : null}
          {status.label}
        </span>
      ) : null}
      {!net.granted && !net.waiting ? (
        <button
          type="button"
          className={cn(GLASS_CHIP, "h-[30px] shrink-0 px-3.5 text-[13.5px]")}
          onClick={() => void net.allow().catch(toastError("Couldn't ask macOS"))}
        >
          {localNetworkButton(net.denied)}
        </button>
      ) : null}
    </div>
  );
}

export function AppPermissionList({
  perms,
  variant,
  title = "Permissions",
  footer,
  inset,
  extra,
  className,
}: {
  perms: PermissionsView;
  variant: "hero" | "settings";
  /** The settings section's heading. */
  title?: string;
  /** Under the rows; the hero's default says what Allow does. */
  footer?: ReactNode;
  /** Hero rows inside another card (the next steps' row): no card of their own. */
  inset?: boolean;
  /** More hero rows in the same card, after these two (Local Network on a Mac that runs macOS Spaces). */
  extra?: ReactNode;
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
              value={permissionStatus(p, Boolean(perms.waiting))?.label ?? "Missing"}
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
              <PermissionStatus
                permission={p}
                waiting={Boolean(perms.waiting)}
                className={cn("pr-1 text-[13.5px] font-medium", p.granted ? "text-white" : GREY)}
              />
              {!p.granted && p.settingsUrl ? (
                <button type="button" className={cn(GLASS_CHIP, "h-[30px] shrink-0 px-3.5 text-[13.5px]")} onClick={() => allow(p.id)}>
                  Allow
                </button>
              ) : null}
            </div>
          );
        })}
        {extra}
      </div>
      <p className={cn(inset ? "mt-1 px-3 pb-1" : "mt-3 px-1", "max-w-[480px] text-[13.5px] leading-[19px] font-medium", GREY)}>
        {footer ??
          allowHint(
            data.appName,
            data.permissions.every((p) => p.granted),
          )}
      </p>
    </div>
  );
}

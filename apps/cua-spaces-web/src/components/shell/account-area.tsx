// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The sidebar's account area (CUA-2204): one line, the avatar's initial
 * and the name (or email), that opens a menu with Settings, Plan (with the
 * plan's word) and Sign out; and, on Free only, a calm "Run more Spaces
 * at once with Pro" banner right above it. The menu's items and the
 * banner's rule are the core's (`account.menu`, `upsell.proBanner`): the
 * banner has Upgrade and no way to close it, and a paid plan never sees it.
 */

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { useNavigate } from "@tanstack/react-router";
import { ChevronsUpDownIcon } from "lucide-react";
import { useMemo } from "react";

import {
  accountMenu,
  proBanner,
  upgradeLink,
  useBridge,
  useRelayUsage,
  useRelayUsageView,
  useSession,
  useSettings,
  useTrackPlanClick,
  type AccountMenuItem,
} from "@/bridge";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

/** Where Plan and Upgrade go: the plan card's link (Pro's checkout on
 * Free), else the account app; with `?from=app_account_menu`. */
function usePlanLink(): { plan: string; open(): void } {
  const { core } = useBridge();
  const { openExternal } = useSession();
  const view = useRelayUsageView();
  const usage = useRelayUsage();
  const accountUrl = useSettings().data?.accountUrl ?? null;
  const trackPlanClick = useTrackPlanClick();
  const url = view?.planCard.primary?.url ?? usage?.upgradeUrl ?? accountUrl;
  return {
    plan: usage?.plan ?? "",
    open: () => {
      trackPlanClick("account_menu");
      if (url) void openExternal(upgradeLink(core, url, "app_account_menu")).catch(() => {});
    },
  };
}

/** The Pro line and Upgrade: Free only (the core's `upsell.proBanner`). */
export function ProBanner() {
  const { core } = useBridge();
  const usage = useRelayUsage();
  const link = usePlanLink();
  const banner = useMemo(() => (core.status !== "ready" || !usage ? null : proBanner(core, { plan: usage.plan })), [core, usage]);
  if (!banner?.visible) return null;
  return (
    <div data-pro-banner="" className="mx-2.5 mb-1.5 flex min-h-9 items-center gap-1 rounded-lg bg-brand-surface py-1 pr-1 pl-3">
      <span data-pro-copy="" className="min-w-0 flex-1 whitespace-normal text-xs leading-4 font-medium">{banner.text}</span>
      <Button size="sm" variant="ghost" data-pro-upgrade="" className="h-6 bg-background/60 px-2 hover:bg-background dark:bg-white/10 dark:hover:bg-white/15" onClick={link.open}>
        {banner.button}
      </Button>
    </div>
  );
}

/** The signed-in line and its menu. `identity`: the name, else the email. */
export function AccountLine({ identity }: { identity: string }) {
  const { core } = useBridge();
  const { data: session, signOut } = useSession();
  const navigate = useNavigate();
  const link = usePlanLink();
  const items: AccountMenuItem[] = useMemo(() => {
    const input = { plan: link.plan, signedIn: true, authMode: session?.account.authMode ?? "" };
    return core.status === "ready"
      ? accountMenu(core, input)
      : [
          { id: "settings", label: "Settings", value: null, separatorBefore: false },
          ...(input.authMode === "user" ? [{ id: "signOut" as const, label: "Sign out", value: null, separatorBefore: true }] : []),
        ];
  }, [core, link.plan, session?.account.authMode]);
  const run = (item: AccountMenuItem) => {
    switch (item.id) {
      case "settings":
        return void navigate({ to: "/settings" });
      case "plan":
        return link.open();
      case "signOut":
        return void signOut().catch((e: unknown) => toast("Couldn't sign out", { description: e instanceof Error ? e.message : String(e) }));
    }
  };
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger
        data-account-line=""
        className="flex h-9 w-full items-center gap-2.5 rounded-lg px-2 text-left outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-foreground/[0.07] dark:data-popup-open:bg-white/[0.08]"
      >
        <span aria-hidden className="flex size-6 shrink-0 items-center justify-center rounded-full bg-brand-surface text-2xs font-semibold text-brand-strong uppercase">
          {identity.slice(0, 1)}
        </span>
        <span className="min-w-0 flex-1 truncate text-xs font-medium" data-signin-identity="">
          {identity}
        </span>
        <ChevronsUpDownIcon aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner side="top" align="start" sideOffset={6} className="z-50 w-(--anchor-width) outline-none">
          <MenuPrimitive.Popup
            data-account-menu=""
            className="glass origin-(--transform-origin) rounded-lg border p-1 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0"
          >
            {items.map((item) => (
              <div key={item.id}>
                {item.separatorBefore ? <MenuPrimitive.Separator className="mx-1 my-1 h-px bg-foreground/10" /> : null}
                <MenuPrimitive.Item
                  data-account-item={item.id}
                  onClick={() => run(item)}
                  className="flex cursor-default items-center gap-3 rounded-md px-2 py-1 text-[13px] outline-none data-highlighted:bg-brand data-highlighted:text-white"
                >
                  <span className="flex-1">{item.label}</span>
                  {item.value ? <span className={cn("text-xs text-muted-foreground", "in-data-highlighted:text-white/80")}>{item.value}</span> : null}
                </MenuPrimitive.Item>
              </div>
            ))}
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}

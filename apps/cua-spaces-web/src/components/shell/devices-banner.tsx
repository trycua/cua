// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useLocation, useNavigate } from "@tanstack/react-router";
import { CircleAlertIcon, TriangleAlertIcon } from "lucide-react";

import { useDevices, useSession } from "@/bridge";
import { Button } from "@/components/ui/button";
import { shellDevicesBanner } from "@/lib/devices";
import { cn } from "@/lib/utils";

/**
 * This device needs enrolling or re-verifying: one bar above every page,
 * the core's words and its Enroll… (the SwiftUI app's `DevicesBanner`).
 * Settings, Devices says the same under its own row, so the bar stays off
 * that page.
 */
export function DevicesBanner() {
  const { data: session } = useSession();
  const devices = useDevices();
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const banner = shellDevicesBanner(devices.data, session?.signedIn);
  if (!banner || pathname.startsWith("/settings/devices")) return null;

  const critical = banner.tone === "critical";
  const Icon = critical ? TriangleAlertIcon : CircleAlertIcon;
  // The enroll sheet is the Devices page's.
  const enroll = () => {
    devices.startEnroll();
    void navigate({ to: "/settings/devices" });
  };
  return (
    <div role="status" data-shell-devices-banner={banner.tone} className="flex shrink-0 items-center gap-2 border-b px-4 py-1.5 text-xs">
      <Icon className={cn("size-3.5 shrink-0", critical ? "text-destructive" : "text-warning")} />
      <span className="min-w-0 flex-1 truncate" title={banner.text}>
        {banner.text}
      </span>
      {banner.actionLabel ? (
        <Button size="sm" variant="outline" className="h-6" onClick={enroll} data-shell-banner-enroll>
          {banner.actionLabel}
        </Button>
      ) : null}
    </div>
  );
}

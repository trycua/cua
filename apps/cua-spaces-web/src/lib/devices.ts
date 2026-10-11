// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Words the Devices page formats itself (the core leaves dates to the shell,
// as the SwiftUI app's `DevicesModel` does).

import type { DeviceRow, DevicesLabels, ThisDevice } from "@/bridge";

/** "Oct 27, 2026" (a medium date, no time). */
export function dateText(secs: number, locale?: string): string {
  return new Intl.DateTimeFormat(locale ?? "en-US", { dateStyle: "medium" }).format(new Date(secs * 1000));
}

/** "just now" within a minute, else "5 minutes ago", "3 days ago". */
export function relativeText(secs: number, nowSecs: number, locale?: string): string {
  const diff = secs - nowSecs;
  if (diff > -60) return "just now";
  const f = new Intl.RelativeTimeFormat(locale ?? "en-US", { numeric: "auto" });
  const abs = Math.abs(diff);
  if (abs < 3600) return f.format(Math.round(diff / 60), "minute");
  if (abs < 86_400) return f.format(Math.round(diff / 3600), "hour");
  if (abs < 30 * 86_400) return f.format(Math.round(diff / 86_400), "day");
  if (abs < 365 * 86_400) return f.format(Math.round(diff / (30 * 86_400)), "month");
  return f.format(Math.round(diff / (365 * 86_400)), "year");
}

/** "Enrolled until Oct 27, 2026" (`DevicesModel.thisDeviceText`). */
export function thisDeviceText(t: ThisDevice, locale?: string): string {
  return t.at === null ? t.title : `${t.title} ${dateText(t.at, locale)}`;
}

/** A row's second line: platform and state, then when it was last seen (`DevicesModel.rowDetail`). */
export function rowDetail(r: DeviceRow, labels: DevicesLabels, nowSecs: number, locale?: string): string {
  return r.lastSeen === null ? r.detail : `${r.detail} · ${labels.lastSeen} ${relativeText(r.lastSeen, nowSecs, locale)}`;
}

export type DeviceKind = "laptop" | "desktop" | "phone" | "unknown";

/** Which icon a platform gets (`DevicesSettingsView.symbol`). */
export function deviceKind(platform: string): DeviceKind {
  switch (platform) {
    case "macOS":
      return "laptop";
    case "Windows":
    case "Linux":
    case "FreeBSD":
      return "desktop";
    case "iOS":
    case "Android":
      return "phone";
    default:
      return "unknown";
  }
}

/** What the Devices tab draws: never nothing (a slow relay read once left it blank). */
export type DevicesPageState = "unsupported" | "signed-out" | "error" | "loading" | "no-view" | "ready";

export function devicesPageState(
  r: { unsupported?: boolean; error: Error | null; data?: { view: unknown } | undefined },
  signedIn: boolean | undefined,
): DevicesPageState {
  if (r.unsupported) return "unsupported";
  if (signedIn === false) return "signed-out";
  if (r.error && !r.data) return "error";
  if (!r.data) return "loading";
  return r.data.view ? "ready" : "no-view";
}

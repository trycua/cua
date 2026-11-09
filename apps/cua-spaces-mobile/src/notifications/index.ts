// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Local notifications for "needs you", with Allow and Deny on the
 * notification itself and a deep link to the exact thread entry.
 *
 * These are local: they fire while the app is running (foreground, or kept
 * alive by iOS for a short time after). A push that reaches a closed app
 * needs a sender in the relay and an APNs key (docs/SETUP.md); until
 * that exists the Settings page says so.
 */

import * as Notifications from "expo-notifications";
import { Platform } from "react-native";

import { approvalsView } from "../core/phone";
import type { SpaceList } from "../data/types";

export const CATEGORY = "needs-you";
/** The category a push from the relay uses (`aps.category`), with the same two actions. */
export const PUSH_CATEGORY = "CUA_APPROVAL";
export const ACTION_ALLOW = "allow";
export const ACTION_DENY = "deny";

export interface NeedsYouPayload {
  spaceId: string;
  /** The approval's thread entry. */
  seq: number | null;
  kind: "approval" | "question";
}

/** The deep link a tap opens: the Space's thread, scrolled to the entry. */
export function linkFor(p: { spaceId: string; seq: number | null }): string {
  const id = encodeURIComponent(p.spaceId);
  return p.seq != null ? `/space/${id}?entry=${p.seq}` : `/space/${id}`;
}

export async function setUpNotifications(): Promise<void> {
  Notifications.setNotificationHandler({
    handleNotification: async () => ({ shouldShowBanner: true, shouldShowList: true, shouldPlaySound: false, shouldSetBadge: false }),
  });
  // The two buttons on the alert say what the card says (the core's Allow once and Decline); both ask to unlock first.
  const buttons = approvalsView({ pending: [{ seq: 0, kind: "", action: "", scopeChoices: ["once"] }] })?.buttons ?? [];
  const allow = buttons.find((b) => b.id === "allow-once")?.label ?? "Allow";
  const decline = buttons.find((b) => b.id === "decline")?.label ?? "Decline";
  const actions = [
    { identifier: ACTION_ALLOW, buttonTitle: allow, options: { opensAppToForeground: false, isAuthenticationRequired: true } },
    { identifier: ACTION_DENY, buttonTitle: decline, options: { opensAppToForeground: false, isDestructive: true } },
  ];
  await Notifications.setNotificationCategoryAsync(CATEGORY, actions).catch(() => {});
  await Notifications.setNotificationCategoryAsync(PUSH_CATEGORY, actions).catch(() => {});
  if (Platform.OS === "android") {
    await Notifications.setNotificationChannelAsync("needs-you", { name: "Needs you", importance: Notifications.AndroidImportance.HIGH }).catch(() => {});
  }
}

export async function permissionState(): Promise<"granted" | "denied" | "undetermined"> {
  const p = await Notifications.getPermissionsAsync();
  return p.granted ? "granted" : p.canAskAgain ? "undetermined" : "denied";
}

export async function askPermission(): Promise<boolean> {
  const p = await Notifications.requestPermissionsAsync({ ios: { allowAlert: true, allowBadge: true, allowSound: false } });
  return p.granted;
}

/** What `needs you` entries a Space list holds, keyed so each one notifies once. */
export function needsYouIn(
  list: SpaceList,
  membersNeeds: Map<string, { seq: number; kind: "approval" | "question"; text: string }[]>,
): { key: string; spaceName: string; payload: NeedsYouPayload; text: string }[] {
  const out: { key: string; spaceName: string; payload: NeedsYouPayload; text: string }[] = [];
  for (const s of list.spaces) {
    if (s.tone !== "you") continue;
    const needs = membersNeeds.get(s.id) ?? [];
    if (!needs.length) out.push({ key: `${s.id}#`, spaceName: s.name, payload: { spaceId: s.id, seq: null, kind: "approval" }, text: s.statusLine });
    for (const n of needs) out.push({ key: `${s.id}#${n.seq}`, spaceName: s.name, payload: { spaceId: s.id, seq: n.seq, kind: n.kind }, text: n.text });
  }
  return out;
}

export async function postNeedsYou(n: { spaceName: string; payload: NeedsYouPayload; text: string }): Promise<void> {
  await Notifications.scheduleNotificationAsync({
    content: {
      title: n.spaceName,
      body: n.text,
      data: n.payload as unknown as Record<string, unknown>,
      categoryIdentifier: n.payload.kind === "approval" ? CATEGORY : undefined,
      interruptionLevel: "timeSensitive",
    },
    trigger: Platform.OS === "android" ? { channelId: "needs-you" } : null,
  });
}

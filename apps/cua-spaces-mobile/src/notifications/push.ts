// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Push to a closed app, the phone's half. It asks the system for this phone's push token, tells the relay about it
 * (`PUT /v1/push/registrations/{device}`), withdraws it when notifications go off or the person signs out, and routes a
 * tap on a push to the thread or the approval it is about. What to send, when, and where a tap goes are the app core's
 * (`push.registration`, `push.route`); the relay's sender, and the APNs key it needs, are not here (docs/SETUP.md).
 * Everything is tested against a mock push provider (`mockPush.ts`) that delivers the same payload shape.
 */

import AsyncStorage from "@react-native-async-storage/async-storage";
import * as Notifications from "expo-notifications";
import { Platform } from "react-native";
import { create } from "zustand";

import { pushRegistration, pushRoute, type PushRoute } from "../core/phone";
import type { Backend } from "../data/types";

const FINGERPRINT_KEY = "cua-spaces-mobile.push.v1";

export interface PushState {
  /** The relay has this phone's token. */
  registered: boolean;
  /** The relay refused or does not know push registrations: Settings says push is not set up for this account yet. */
  unavailable: boolean;
  /** The relay can send push (a provider, the APNs key, is set up). Without it a registered token still gets nothing. */
  configured: boolean;
  set(p: Partial<Pick<PushState, "registered" | "unavailable" | "configured">>): void;
}

export const usePushState = create<PushState>((set) => ({ registered: false, unavailable: false, configured: true, set: (p) => set(p) }));

/** This phone's APNs token as hex, or null where the system gives none (the Simulator without a push entitlement, Android here). */
export async function getPushToken(): Promise<string | null> {
  if (Platform.OS !== "ios") return null;
  try {
    const t = await Notifications.getDevicePushTokenAsync();
    return typeof t.data === "string" ? t.data : null;
  } catch {
    return null;
  }
}

export interface SyncInput {
  backend: Backend;
  deviceId: string | null;
  permission: "granted" | "denied" | "undetermined";
  needsYou: boolean;
  done: boolean;
  /** The token source (tests give a mock). */
  token?: () => Promise<string | null>;
  env?: "sandbox" | "production";
  locale?: string;
  appId?: string;
  storage?: Pick<typeof AsyncStorage, "getItem" | "setItem" | "removeItem">;
}

/**
 * Makes the relay's registration match what this phone wants: registered once with the current switches, withdrawn
 * when it no longer wants push. A relay that refuses is remembered as "not set up" (the app keeps its local alerts).
 */
export async function syncPush(i: SyncInput): Promise<"registered" | "unregistered" | "unchanged" | "unavailable"> {
  const push = i.backend.push;
  if (!push || !i.backend.capabilities.push) {
    usePushState.getState().set({ registered: false, unavailable: true });
    return "unavailable";
  }
  // Does the relay send push at all? A relay without push routes (or without a provider) says so, and Settings says it.
  try {
    const st = push.status ? await push.status() : { configured: true };
    usePushState.getState().set({ configured: st.configured, unavailable: false });
  } catch {
    usePushState.getState().set({ registered: false, unavailable: true });
    return "unavailable";
  }
  const kv = i.storage ?? AsyncStorage;
  const registered = await kv.getItem(FINGERPRINT_KEY).catch(() => null);
  const token = i.permission === "granted" && (i.needsYou || i.done) ? await (i.token ?? getPushToken)() : null;
  const plan = pushRegistration({
    permission: i.permission,
    token,
    deviceId: i.deviceId ?? "",
    env: i.env ?? (__DEV__ ? "sandbox" : "production"),
    appId: i.appId ?? "ai.cua.spaces.mobile",
    locale: i.locale ?? "en",
    needsYou: i.needsYou,
    done: i.done,
    registered,
  });
  if (!plan) return "unchanged";
  try {
    if (plan.action === "register" && plan.body && plan.fingerprint) {
      await push.register(plan.body);
      await kv.setItem(FINGERPRINT_KEY, plan.fingerprint).catch(() => {});
      usePushState.getState().set({ registered: true, unavailable: false });
      return "registered";
    }
    if (plan.action === "unregister") {
      await push.unregister();
      await kv.removeItem(FINGERPRINT_KEY).catch(() => {});
      usePushState.getState().set({ registered: false });
      return "unregistered";
    }
  } catch {
    // The relay has no push for this account (or cannot be reached): alerts stay local, and Settings says so.
    usePushState.getState().set({ registered: false, unavailable: true });
    return "unavailable";
  }
  usePushState.getState().set({ registered: registered !== null });
  return "unchanged";
}

/** Sign-out: the relay stops pushing to this phone. */
export async function withdrawPush(backend: Backend, storage: Pick<typeof AsyncStorage, "removeItem"> = AsyncStorage): Promise<void> {
  try {
    await backend.push?.unregister();
  } catch {
    /* the relay forgets a phone whose session ended */
  }
  await storage.removeItem(FINGERPRINT_KEY).catch(() => {});
  usePushState.getState().set({ registered: false });
}

/** Where a tap on a push goes: the approval or the thread it is about, else Home. */
export function routeFor(data: unknown): PushRoute | null {
  return pushRoute(data);
}

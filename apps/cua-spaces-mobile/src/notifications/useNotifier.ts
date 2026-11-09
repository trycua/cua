// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Notifications from "expo-notifications";
import { useRouter } from "expo-router";
import { useEffect, useRef } from "react";

import { useApp } from "../state/app";
import { useBackend, useForeground, useSpaceList } from "../state/backend";
import { usePrefs } from "../state/prefs";
import { useOutbox } from "../state/stores";
import { ACTION_ALLOW, ACTION_DENY, needsYouIn, permissionState, postNeedsYou, setUpNotifications } from "./index";
import { routeFor, syncPush } from "./push";

/**
 * Watches the Space list: each new "needs you" posts one local notification. A push from the relay (or a local one)
 * opens the approval or the thread it is about; Allow and Decline on the alert answer it through the outbox, so an
 * answer made offline waits and goes once. The phone's push registration follows the switches in Settings.
 */
export function useNotifier(): void {
  const backend = useBackend();
  const router = useRouter();
  const outbox = useOutbox();
  const { list } = useSpaceList();
  const { deviceId } = useApp();
  const needsYouOn = usePrefs((p) => p.notifyNeedsYou);
  const doneOn = usePrefs((p) => p.notifyDone);
  const seen = useRef<Set<string> | null>(null);
  const foreground = useForeground();

  useEffect(() => {
    setUpNotifications().catch(() => {});
  }, []);

  // The relay knows where to push for this phone, and stops when the switches go off.
  useEffect(() => {
    let alive = true;
    void (async () => {
      const permission = await permissionState().catch(() => "undetermined" as const);
      if (alive) await syncPush({ backend, deviceId: deviceId(), permission, needsYou: needsYouOn, done: doneOn });
    })();
    return () => {
      alive = false;
    };
  }, [backend, deviceId, needsYouOn, doneOn, foreground]);

  useEffect(() => {
    if (!list || !needsYouOn) return;
    // Only entries that appear after the app has the first list notify: opening the app is not news.
    const first = seen.current === null;
    seen.current ??= new Set();
    (async () => {
      const needs = new Map<string, { seq: number; kind: "approval" | "question"; text: string }[]>();
      for (const s of list.spaces.filter((x) => x.tone === "you")) {
        const ms = await backend.members(s.id).catch(() => []);
        needs.set(
          s.id,
          ms.flatMap((m) => (m.needsYou ?? []).map((n) => ({ seq: n.seq, kind: n.kind, text: n.text }))),
        );
      }
      for (const n of needsYouIn(list, needs)) {
        if (seen.current!.has(n.key)) continue;
        seen.current!.add(n.key);
        if (!first && (await permissionState()) === "granted") await postNeedsYou(n).catch(() => {});
      }
    })();
  }, [list, needsYouOn, backend]);

  useEffect(() => {
    const open = (r: Notifications.NotificationResponse) => {
      const data = r.notification.request.content.data as unknown;
      const route = routeFor(data);
      if (!route || route.target === "home" || !route.spaceId) return;
      if (r.actionIdentifier === ACTION_ALLOW || r.actionIdentifier === ACTION_DENY) {
        // Answered from the alert: the same outbox as the card, so offline it waits and goes once.
        if (route.approval != null) {
          const extra = { approval: route.approval, approve: r.actionIdentifier === ACTION_ALLOW, scope: "once" };
          if (outbox) void outbox.send({ space: route.spaceId, kind: "answer", extra });
          else backend.answer(route.spaceId, route.approval, r.actionIdentifier === ACTION_ALLOW).catch(() => {});
        }
        return;
      }
      router.push(route.link as never);
    };
    const sub = Notifications.addNotificationResponseReceivedListener(open);
    // The app was closed and a tap on a push opened it: that tap is the last response.
    void Notifications.getLastNotificationResponseAsync?.()
      ?.then((r) => {
        if (r && !handledCold.has(r.notification.request.identifier)) {
          handledCold.add(r.notification.request.identifier);
          open(r);
        }
      })
      .catch(() => {});
    return () => sub.remove();
  }, [backend, router, outbox]);
}

/** A cold-start response is routed once per notification, however many times the screen mounts. */
const handledCold = new Set<string>();

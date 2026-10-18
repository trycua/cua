// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState, type RefObject } from "react";

import { nativeVideoHandler } from "@/lib/stream-surface";
import type { SlotOptions, SlotStatus, VideoSlotReporter } from "@/lib/video-slots";

import { NO_VIDEO_HERE, webCodecsHost } from "./webcodecs-host";

export interface VideoSlot extends SlotStatus {
  /** Give keys to this slot's Space (true) or back to the page (false). */
  setFocus(focused: boolean): void;
}

type Slots = Pick<VideoSlotReporter, "register" | "focus" | "idOf">;

// The reporter loads only where the host draws native video, and the
// WebCodecs slots only in the Electron shell (webcodecs-slots.ts), so the
// browser downloads neither.
let reporter: Slots | null = null;
let loading: Promise<Slots | null> | null = null;

function loadReporter(): Promise<Slots | null> {
  loading ??= (nativeVideoHandler() !== null
    ? import("@/lib/video-slots").then((m) => m.videoSlots())
    : import("./webcodecs-slots").then((m) => m.webCodecsSlots())
  ).then((r) => (reporter = r));
  return loading;
}

/** Tests: forget the loaded reporter. */
export function resetVideoSlotLoader(): void {
  reporter = null;
  loading = null;
}

const CONNECTING: SlotStatus = { phase: "connecting", focused: false };

/** The host draws live video in the page at all (native slots, or the
 * Electron shell's WebCodecs): a Space's preview can connect. */
export function videoHostPresent(): boolean {
  return nativeVideoHandler() !== null || webCodecsHost() !== null;
}

/**
 * While `active` and the host draws native video, reports `ref`'s rect so
 * the host draws the Space's live video over it, and answers how that is
 * going (connecting, live, failed) and whether keys go to the Space. Null
 * when the host draws no video (the browser, Electron, the experiment off)
 * or `active` is false: show the fallback.
 */
export function useVideoSlot(
  ref: RefObject<HTMLElement | null>,
  { active, spaceId, tier, interactive, radius, inset = 0, os, windowId, epoch, attempt = 0 }: SlotOptions & {
    active: boolean;
    /** A new value opens the slot again (Try again after a failure). */
    attempt?: number;
  },
): VideoSlot | null {
  const on = active && videoHostPresent();
  const [slots, setSlots] = useState<Slots | null>(reporter);
  const [status, setStatus] = useState<SlotStatus>(CONNECTING);
  const [id, setId] = useState<string | null>(null);

  useEffect(() => {
    if (!on || slots) return;
    let live = true;
    void loadReporter().then((r) => live && setSlots(r));
    return () => {
      live = false;
    };
  }, [on, slots]);

  useEffect(() => {
    const el = ref.current;
    if (!on || !el || !slots) return;
    setStatus(CONNECTING);
    const stop = slots.register(el, { spaceId, tier, interactive, radius, inset, os, windowId, epoch }, setStatus);
    setId(slots.idOf(el));
    return () => {
      stop();
      setId(null);
    };
  }, [ref, on, slots, spaceId, tier, interactive, radius, inset, os, windowId, epoch, attempt]);

  // The shell has no video for this page (no cua, sample data): the plain fallback, as in a browser.
  if (!on || status.reason === NO_VIDEO_HERE) return null;
  return { ...status, setFocus: (focused) => slots?.focus(focused ? id : null) };
}

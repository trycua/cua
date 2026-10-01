// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The onboarding miniatures' shared loop clock (PresentationPreview,
 * DriverPreview): reduced motion, and milliseconds into a loop that only
 * ticks while the miniature is on screen in a visible, focused window.
 */
import { type RefObject, useEffect, useState } from "react";

/** Whether the system asks for reduced motion (live). */
export function useReducedMotion(): boolean {
  const query = "(prefers-reduced-motion: reduce)";
  const [reduced, setReduced] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const mq = window.matchMedia(query);
    const on = () => setReduced(mq.matches);
    mq.addEventListener?.("change", on);
    return () => mq.removeEventListener?.("change", on);
  }, []);
  return reduced;
}

/**
 * Milliseconds into the loop, ticking at about 30 fps while `run` and the
 * stage is on screen in a visible, focused window; frozen otherwise.
 */
export function useLoopTime(ref: RefObject<HTMLDivElement | null>, run: boolean, loopMs: number): number {
  const [t, setT] = useState(0);
  useEffect(() => {
    if (!run) return;
    let visible = true;
    let raf = 0;
    let last = 0;
    let elapsed = 0;
    let prev: number | null = null;
    const active = () => visible && document.visibilityState === "visible" && document.hasFocus();
    const tick = (now: number) => {
      if (prev !== null) elapsed += now - prev;
      prev = now;
      if (now - last >= 1000 / 30) {
        last = now;
        setT(elapsed % loopMs);
      }
      raf = requestAnimationFrame(tick);
    };
    const update = () => {
      if (active() && !raf) {
        prev = null;
        raf = requestAnimationFrame(tick);
      } else if (!active() && raf) {
        cancelAnimationFrame(raf);
        raf = 0;
      }
    };
    const io =
      typeof IntersectionObserver === "undefined"
        ? null
        : new IntersectionObserver((entries) => {
            visible = entries.some((e) => e.isIntersecting);
            update();
          });
    if (io && ref.current) io.observe(ref.current);
    document.addEventListener("visibilitychange", update);
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    update();
    return () => {
      io?.disconnect();
      document.removeEventListener("visibilitychange", update);
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      if (raf) cancelAnimationFrame(raf);
    };
  }, [ref, run, loopMs]);
  return t;
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState, type RefObject } from "react";

/** How far outside the viewport a tile already counts as shown, so it has
 * its picture by the time it scrolls in. */
export const IN_VIEW_MARGIN = "400px";

/**
 * Whether `ref`'s element is on screen (or within `margin` of it). A grid of
 * many Spaces asks for pictures and live video only for the tiles near the
 * viewport, so 200 Spaces cost what the dozen on screen cost. True where
 * the page cannot tell (no IntersectionObserver), as before.
 */
export function useInView(ref: RefObject<Element | null>, margin = IN_VIEW_MARGIN): boolean {
  const [inView, setInView] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) setInView(e.isIntersecting);
    }, { rootMargin: margin });
    io.observe(el);
    return () => io.disconnect();
  }, [ref, margin]);
  return inView;
}

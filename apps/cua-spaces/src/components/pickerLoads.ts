// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useRef, useState } from "react";

/**
 * The picker grid's icon and preview loads: each key is asked once, a few
 * at a time, in the order asked (the tiles in grid order, so the visible
 * ones first), and results are applied once per frame, so the grid redraws
 * a few times rather than once per icon.
 */
export function useGridLoads<V>(limit = 8): {
  values: Map<string, V | null>;
  request: (key: string, load: () => Promise<V | null>) => void;
} {
  const [values, setValues] = useState<Map<string, V | null>>(() => new Map());
  const asked = useRef(new Set<string>());
  const queue = useRef<{ key: string; load: () => Promise<V | null> }[]>([]);
  const running = useRef(0);
  const pending = useRef(new Map<string, V | null>());
  const frame = useRef<ReturnType<typeof setTimeout> | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const flush = useCallback(() => {
    frame.current = null;
    if (!alive.current || pending.current.size === 0) return;
    const done = pending.current;
    pending.current = new Map();
    setValues((m) => {
      const next = new Map(m);
      done.forEach((v, k) => next.set(k, v));
      return next;
    });
  }, []);

  const pump = useCallback(() => {
    while (running.current < limit && queue.current.length > 0) {
      const { key, load } = queue.current.shift()!;
      running.current++;
      void load()
        .catch(() => null)
        .then((value) => {
          running.current--;
          if (!alive.current) return;
          pending.current.set(key, value ?? null);
          // A timer, not requestAnimationFrame: that one pauses while the
          // window is hidden.
          if (frame.current === null) frame.current = setTimeout(flush, 16);
          pump();
        });
    }
  }, [limit, flush]);

  const request = useCallback(
    (key: string, load: () => Promise<V | null>) => {
      if (!key || asked.current.has(key)) return;
      asked.current.add(key);
      queue.current.push({ key, load });
      pump();
    },
    [pump],
  );

  return { values, request };
}

// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

/**
 * True once `signature` (a string of everything the page reads from the host) has stopped changing for
 * `quietMs`, or after `capMs` at the latest. A page's cards and lists come from several host reads that
 * answer one after another; drawn as each arrives they push each other down. A page that draws its body when
 * this turns true shows the finished layout in one step (CUA-1951). It stays true afterwards, so later
 * updates change content in place as before.
 */
export function useQuiet(signature: string, quietMs = 120, capMs = 700): boolean {
  const [quiet, setQuiet] = useState(false);
  useEffect(() => {
    if (quiet) return;
    const t = setTimeout(() => setQuiet(true), quietMs);
    return () => clearTimeout(t);
  }, [signature, quiet, quietMs]);
  useEffect(() => {
    const t = setTimeout(() => setQuiet(true), capMs);
    return () => clearTimeout(t);
  }, [capMs]);
  return quiet;
}

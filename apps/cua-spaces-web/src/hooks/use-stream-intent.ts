// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef } from "react";

import { useSettings } from "@/bridge";
import { watchStreamIntent } from "@/lib/stream-intent";

/** Opens a Space's stream when the pointer heads for it (`lib/stream-intent.ts`),
 * unless the person turned off connecting by itself. */
export function useStreamIntent(): void {
  const settings = useSettings();
  const autoConnect = settings.data?.values.autoConnect ?? true;
  const allowed = useRef(autoConnect);
  allowed.current = autoConnect;
  useEffect(() => watchStreamIntent(document, { allowed: () => allowed.current }), []);
}
